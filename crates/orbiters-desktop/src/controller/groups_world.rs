//! A made-up VRChat for the groups' tests: one group with its roles, members, posts and pictures, the account's own files;
//! it answers reads from its state, carries writes out on it, and can fail a write in any way the network can, done or
//! not done. Nothing of it reaches the network; nobody in it exists.
use super::*;
use crate::controller::Shared;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_api::{Routed, Sent};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};

pub(crate) const G: &str = "grp_00000000-0000-4000-8000-0000000000aa";
/// Another member (no role), one who moderates, and a namesake of the first.
pub(crate) const U: &str = "usr_00000000-0000-4000-8000-0000000000bb";
pub(crate) const V: &str = "usr_00000000-0000-4000-8000-0000000000be";
pub(crate) const TWIN: &str = "usr_00000000-0000-4000-8000-0000000000cc";
/// Words and an author that are someone else's: never on this computer's disk.
pub(crate) const OTHERS_WORDS: &str = "OTHER_MODERATOR_ORIGINAL";
pub(crate) const OTHERS_ID: &str = "usr_00000000-0000-4000-8000-0000000000dd";

/// Holds a request in flight until let go.
#[derive(Default)]
pub(crate) struct Hold {
    reached: Mutex<bool>,
    open: Mutex<bool>,
    cv: Condvar,
}
impl Hold {
    fn wait(&self) {
        *self.reached.lock().unwrap() = true;
        self.cv.notify_all();
        let _ = self.cv.wait_timeout_while(self.open.lock().unwrap(), Duration::from_secs(20), |o| !*o);
    }
    pub(crate) fn until_reached(&self) -> bool {
        let (r, _) = self.cv.wait_timeout_while(self.reached.lock().unwrap(), Duration::from_secs(10), |r| !*r).unwrap();
        *r
    }
    pub(crate) fn let_go(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }
}

/// How the next write fails.
#[derive(Clone)]
pub(crate) enum Fault {
    /// Carried out or not, and this answer.
    Answer { done: bool, status: u16, body: Value },
    /// Carried out or not, and an answer that is not JSON.
    Garbage { done: bool },
    /// Carried out or not, and the connection dies before the answer (a drop, a timeout).
    Drop { done: bool },
    /// Carried out, the answer held back.
    Held(Arc<Hold>),
}

pub(crate) struct World {
    pub rights: Vec<String>,
    pub banner: Value,
    pub icon: Value,
    pub roles: Vec<Value>,
    pub members: BTreeMap<String, Value>,
    pub posts: Vec<Value>,
    /// (kind, the file)
    pub files: Vec<(String, Value)>,
    pub tags: Vec<String>,
    pub fault: Option<Fault>,
    /// Answers to reads whose address holds the text, instead of the state's.
    pub instead: Vec<(String, Value)>,
    /// Run once, when a read whose address holds the text was answered (what others do meanwhile).
    #[allow(clippy::type_complexity)]
    pub then: Vec<(String, Box<dyn FnOnce(&mut World) + Send>)>,
    /// Writes carried out.
    pub done: Vec<String>,
    made: u32,
}

pub(crate) fn role(id: &str, name: &str, order: i64, rights: &[&str], management: bool) -> Value {
    json!({"id": id, "groupId": G, "name": name, "description": "", "order": order, "permissions": rights, "defaultRole": false, "isManagementRole": management,
        "requiresPurchase": false, "isSelfAssignable": false, "isAddedOnJoin": false, "requiresTwoFactor": false})
}
pub(crate) fn member(user: &str, roles: &[&str]) -> Value {
    json!({"id": format!("gmem_{}", &user[user.len() - 12..]), "groupId": G, "userId": user, "roleIds": roles, "mRoleIds": [], "user": {"displayName": "Sam"}})
}
pub(crate) fn post(id: &str, author: &str, title: &str, text: &str, visibility: &str, roles: &[&str], image: Value) -> Value {
    json!({"id": id, "groupId": G, "authorId": author, "title": title, "text": text, "visibility": visibility, "roleIds": roles, "imageId": image, "createdAt": "2026-01-01T00:00:00Z"})
}
/// A picture's bytes: a small PNG, another for every file.
pub(crate) fn picture(id: &str) -> Vec<u8> {
    let c = id.bytes().fold(7u8, |a, b| a.wrapping_mul(31).wrapping_add(b));
    crate::pages::groups::art::encode_png(&image::RgbaImage::from_pixel(6, 4, image::Rgba([c, c.wrapping_mul(3), 90, 255]))).unwrap()
}
pub(crate) fn file(id: &str, tag: &str, owner: &str, bytes: &[u8]) -> Value {
    json!({"id": id, "ownerId": owner, "tags": [tag], "versions": [{"version": 0}, {"version": 1, "created_at": now(), "file": {"url": format!("https://api.vrchat.cloud/api/1/file/{id}/1/file"), "md5": orbiters_core::vrchat_api::group_file_md5(bytes), "sizeInBytes": bytes.len()}}]})
}
/// Now, a second ahead: VRChat's clock.
pub(crate) fn now() -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::seconds(1)).format(&time::format_description::well_known::Rfc3339).unwrap()
}

impl Default for World {
    fn default() -> Self {
        let mut everyone = role("grol_everyone", "Everyone", 9, &["group-instance-join"], false);
        everyone["defaultRole"] = json!(true);
        let g1 = picture("file_gal1");
        let i1 = picture("file_ico1");
        Self {
            rights: vec!["*".into()],
            banner: json!("file_old"),
            icon: Value::Null,
            roles: vec![
                role("grol_owner", "Owner", 0, &["*"], true),
                role("grol_mod", "Moderators", 2, &["group-instance-moderate"], true),
                role("grol_artist", "Artists", 3, &[], false),
                role("grol_events", "Event goers", 4, &[], false),
                everyone,
            ],
            members: [(U, member(U, &[])), (V, member(V, &["grol_mod"])), (TWIN, member(TWIN, &["grol_mod"]))].into_iter().map(|(u, m)| (u.to_string(), m)).collect(),
            posts: vec![post("gpos-1".replace('-', "_").as_str(), OTHERS_ID, "Meet-up", OTHERS_WORDS, "group", &["grol_artist"], json!("file_pic")), post("gpos_2", "usr_test", "Old news", "Nothing", "public", &[], Value::Null)],
            files: vec![("gallery".into(), file("file_gal1", "gallery", "usr_test", &g1)), ("icon".into(), file("file_ico1", "icon", "usr_test", &i1))],
            tags: vec!["system_supporter".into()],
            fault: None,
            instead: Vec::new(),
            then: Vec::new(),
            done: Vec::new(),
            made: 0,
        }
    }
}

impl World {
    fn group(&self) -> Value {
        json!({"id": G, "name": "Made-up Makers", "shortCode": "MAKE", "discriminator": "0001", "ownerId": "usr_test", "memberCount": 120, "bannerId": self.banner, "iconId": self.icon,
            "myMember": {"id": "gmem_me", "groupId": G, "userId": "usr_test", "roleIds": ["grol_owner"], "mRoleIds": [], "permissions": self.rights, "membershipStatus": "member"},
            "roles": self.roles})
    }
    fn read(&mut self, path: &str) -> Routed {
        if let Some((_, v)) = self.instead.iter().find(|(k, _)| path.contains(k.as_str())) {
            return Routed::Answer(200, v.clone());
        }
        let base = format!("/groups/{G}");
        let answer = |v: Value| Routed::Answer(200, v);
        if let Some(rest) = path.strip_prefix(&base) {
            let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
            return match rest.split('/').collect::<Vec<_>>().as_slice() {
                [""] => answer(self.group()),
                ["", "permissions"] => answer(json!(["group-instance-moderate", "group-instance-join", "group-data-manage"].map(|p| json!({"name": p, "displayName": p, "allowedToAdd": true})))),
                ["", "members", u] => answer(self.members.get(*u).cloned().unwrap_or(Value::Null)),
                ["", "members"] => answer(Value::Array(self.members.values().cloned().collect())),
                ["", "posts"] => {
                    let offset: usize = query.split('&').find_map(|q| q.strip_prefix("offset=")).and_then(|o| o.parse().ok()).unwrap_or(0);
                    answer(json!({"posts": self.posts.iter().skip(offset).take(50).collect::<Vec<_>>(), "total": self.posts.len()}))
                }
                _ => Routed::Answer(404, json!({})),
            };
        }
        if path.starts_with("/auth/user") {
            return answer(json!({"id": "usr_test", "displayName": "Tester", "tags": self.tags}));
        }
        if let Some(q) = path.strip_prefix("/files?tag=") {
            let tag = q.split('&').next().unwrap_or("");
            let offset: usize = q.split('&').find_map(|q| q.strip_prefix("offset=")).and_then(|o| o.parse().ok()).unwrap_or(0);
            return answer(Value::Array(self.files.iter().filter(|(t, _)| t == tag).map(|(_, f)| f.clone()).skip(offset).take(50).collect()));
        }
        if let Some(id) = path.strip_prefix("/file/").and_then(|r| r.strip_suffix("/1/file")) {
            return Routed::Bytes(200, picture(id));
        }
        Routed::Answer(404, json!({}))
    }
    /// A write carried out, and VRChat's answer to it.
    fn carry_out(&mut self, r: &Sent, path: &str) -> Value {
        let body: Value = r.body.as_deref().and_then(|b| serde_json::from_str(b).ok()).unwrap_or(Value::Null);
        self.done.push(format!("{} {path}", r.method));
        self.made += 1;
        let parts: Vec<&str> = path.split('/').collect();
        match (r.method.as_str(), parts.as_slice()) {
            (m, ["", "groups", _, "members", u, "roles", role]) => {
                let held = self.members.get_mut(*u).unwrap()["roleIds"].as_array_mut().unwrap();
                held.retain(|x| x != role);
                if m == "PUT" {
                    held.push(json!(role));
                }
                Value::Array(held.clone())
            }
            ("POST", ["", "groups", _, "roles"]) => {
                let mut v = role(&format!("grol_new{}", self.made), "", 5, &[], false);
                for (k, x) in body.as_object().unwrap() {
                    v[k.as_str()] = x.clone();
                }
                v["createdAt"] = json!(now());
                self.roles.push(v.clone());
                v
            }
            ("PUT", ["", "groups", _, "roles", id]) => {
                let found = self.roles.iter_mut().find(|r| r["id"] == *id).unwrap();
                for (k, x) in body.as_object().unwrap() {
                    found[k.as_str()] = x.clone();
                }
                json!(self.roles)
            }
            ("DELETE", ["", "groups", _, "roles", id]) => {
                self.roles.retain(|r| r["id"] != *id);
                json!(self.roles)
            }
            ("POST", ["", "groups", _, "posts"]) => {
                let roles: Vec<String> = body["roleIds"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect()).unwrap_or_default();
                let mut v = post(&format!("gpos_new{}", self.made), "usr_test", body["title"].as_str().unwrap(), body["text"].as_str().unwrap(), body["visibility"].as_str().unwrap(), &roles.iter().map(String::as_str).collect::<Vec<_>>(), body["imageId"].clone());
                v["createdAt"] = json!(now());
                self.posts.insert(0, v.clone());
                v
            }
            ("PUT", ["", "groups", _, "posts", id]) => {
                let found = self.posts.iter_mut().find(|p| p["id"] == *id).unwrap();
                for (k, x) in body.as_object().unwrap() {
                    found[k.as_str()] = x.clone();
                }
                found.clone()
            }
            ("DELETE", ["", "groups", _, "posts", id]) => {
                self.posts.retain(|p| p["id"] != *id);
                json!({"success": {"message": "Group Post was deleted!", "status_code": 200}})
            }
            ("PUT", ["", "groups", _]) => {
                if let Some(b) = body.get("bannerId") {
                    self.banner = b.clone();
                }
                if let Some(i) = body.get("iconId") {
                    self.icon = i.clone();
                }
                self.group()
            }
            ("POST", ["", "file", "image"]) => {
                let (tag, bytes) = r.upload.clone().unwrap();
                let v = file(&format!("file_up{}", self.made), &tag, "usr_test", &bytes);
                self.files.push((tag, v.clone()));
                v
            }
            other => panic!("a write nobody asked for: {other:?}"),
        }
    }
}

pub(crate) type Shown = Arc<Mutex<World>>;

/// VRChat's side of a request.
pub(crate) fn route(world: Shown) -> Arc<dyn Fn(&Sent) -> Routed + Send + Sync> {
    Arc::new(move |r: &Sent| {
        let path = r.url.trim_start_matches("https://api.vrchat.cloud/api/1").to_string();
        if r.method == "GET" {
            let mut w = world.lock().unwrap();
            let answer = w.read(&path);
            if let Some(at) = w.then.iter().position(|(k, _)| path.contains(k.as_str())) {
                let (_, f) = w.then.remove(at);
                f(&mut w);
            }
            return answer;
        }
        let mut w = world.lock().unwrap();
        match w.fault.take() {
            None => Routed::Answer(200, w.carry_out(r, &path)),
            Some(Fault::Answer { done, status, body }) => {
                if done {
                    w.carry_out(r, &path);
                }
                Routed::Answer(status, body)
            }
            Some(Fault::Garbage { done }) => {
                if done {
                    w.carry_out(r, &path);
                }
                Routed::Bytes(200, b"<html>not json</html>".to_vec())
            }
            Some(Fault::Drop { done }) => {
                if done {
                    w.carry_out(r, &path);
                }
                Routed::Drop
            }
            Some(Fault::Held(hold)) => {
                let v = w.carry_out(r, &path);
                drop(w);
                hold.wait();
                Routed::Answer(200, v)
            }
        }
    })
}

/// A controller signed in to a made-up VRChat, that world, and every request it saw.
pub(crate) fn world() -> (Shared, Shown, Arc<Mutex<Vec<Sent>>>) {
    let c = Controller::new(Settings::default());
    let w: Shown = Arc::new(Mutex::new(World::default()));
    let sent = c.test_vrchat_routed(route(w.clone()));
    (c, w, sent)
}
pub(crate) fn scope(c: &Shared) -> Scope {
    Scope::of(&c.view())
}
pub(crate) fn calls(sent: &Arc<Mutex<Vec<Sent>>>) -> Vec<String> {
    sent.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.url.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect()
}
/// The writes that went out.
pub(crate) fn writes(sent: &Arc<Mutex<Vec<Sent>>>) -> Vec<String> {
    calls(sent).into_iter().filter(|c| !c.starts_with("GET")).collect()
}

/// Each kind's write, as the window would ask for it, from what `ask` reads.
pub(crate) fn write_of(kind: Kind, ask: &dyn Fn(Request) -> Reply) -> Write {
    let group = G.to_string();
    let role = |id: &str| match ask(Request::Open { group: G.into() }) {
        Reply::Opened(p) => p.group.roles.unwrap().into_iter().find(|r| r.id == id).unwrap(),
        other => panic!("{other:?}"),
    };
    let post = |id: &str| match ask(Request::Posts { group: G.into(), offset: 0 }) {
        Reply::Posts(p) => p.items.into_iter().find(|p| p.id == id).unwrap(),
        other => panic!("{other:?}"),
    };
    let upload = |tag: FileTag| Write::Upload { tag, group: G.into(), staged: crate::pages::groups::stage_upload(&picture(&uuid::Uuid::new_v4().to_string())).unwrap(), art: (tag == FileTag::Gallery).then_some(42) };
    match kind {
        Kind::AssignRole => Write::MemberRole { group, user: U.into(), role: "grol_mod".into(), add: true },
        Kind::UnassignRole => Write::MemberRole { group, user: V.into(), role: "grol_mod".into(), add: false },
        Kind::CreateRole => Write::CreateRole { group, draft: RoleDraft { name: "Greeters".into(), description: "Say hello".into(), permissions: vec!["group-instance-join".into()], ..Default::default() } },
        Kind::EditRole => {
            let baseline = role("grol_artist");
            let edit = RoleDraft { name: "Painters".into(), requires_two_factor: true, ..RoleDraft::of(&baseline) };
            Write::EditRole { group, baseline, edit }
        }
        Kind::DeleteRole => Write::DeleteRole { group, role: "grol_events".into() },
        Kind::CreatePost => Write::CreatePost { group, draft: PostDraft { title: "Saturday".into(), text: "Bring your art".into(), role_ids: vec!["grol_artist".into()], send_notification: true, ..Default::default() } },
        Kind::EditPost => {
            let baseline = post("gpos_1");
            let edit = PostDraft { title: "Meet-up!".into(), text: baseline.text.clone(), visibility: baseline.visibility, role_ids: baseline.role_ids.clone(), image_id: baseline.image_id.clone(), send_notification: true };
            Write::EditPost { group, baseline, edit }
        }
        Kind::DeletePost => Write::DeletePost { group, post: post("gpos_2") },
        Kind::ApplyBanner => Write::ApplyArt { group, slot: ArtSlot::Banner, file: "file_gal1".into() },
        Kind::ApplyIcon => Write::ApplyArt { group, slot: ArtSlot::Icon, file: "file_ico1".into() },
        Kind::UploadGallery => upload(FileTag::Gallery),
        Kind::UploadIcon => upload(FileTag::Icon),
    }
}

/// What VRChat shows that looks like it, and is not evidence: after each, a check must settle nothing. `done`: whether
/// the write was in fact carried out (the decoy hides it) or not (the decoy imitates it). They change `w` in place.
#[allow(clippy::type_complexity)]
pub(crate) fn decoys(kind: Kind, bytes_md5: Option<String>) -> Vec<(&'static str, Box<dyn Fn(&mut World)>)> {
    let before = "2026-01-01T00:00:00Z";
    let instead = |key: String, v: Value| -> Box<dyn Fn(&mut World)> { Box::new(move |w: &mut World| w.instead = vec![(key.clone(), v.clone())]) };
    let members = |u: &str, has: bool| -> Vec<(&'static str, Box<dyn Fn(&mut World)>)> {
        let held: &[&str] = if has { &["grol_mod"] } else { &[] };
        let key = format!("/members/{u}");
        let mut other_group = member(u, held);
        other_group["groupId"] = json!("grp_00000000-0000-4000-8000-0000000000ff");
        let mut no_managed = member(u, held);
        no_managed.as_object_mut().unwrap().remove("mRoleIds");
        let mut no_roles = member(u, held);
        no_roles.as_object_mut().unwrap().remove("roleIds");
        let mut no_group = member(u, held);
        no_group.as_object_mut().unwrap().remove("groupId");
        vec![
            ("another member's record", instead(key.clone(), member(TWIN, held))),
            ("the record of another group", instead(key.clone(), other_group)),
            ("no group in the record", instead(key.clone(), no_group)),
            ("no managed roles in the record", instead(key.clone(), no_managed)),
            ("no roles in the record", instead(key.clone(), no_roles)),
            ("not a member any more", instead(key, Value::Null)),
        ]
    };
    let art = |slot: &'static str| -> Vec<(&'static str, Box<dyn Fn(&mut World)>)> {
        let set = move |v: Option<Value>| -> Box<dyn Fn(&mut World)> {
            Box::new(move |w: &mut World| {
                let mut g = w.group();
                match &v {
                    Some(v) => g[slot] = v.clone(),
                    None => {
                        g.as_object_mut().unwrap().remove(slot);
                    }
                }
                w.instead = vec![(format!("/groups/{G}?"), g)];
            })
        };
        vec![("the field left out", set(None)), ("an address instead of a file", set(Some(json!("https://elsewhere/x")))), ("a picture someone else put", set(Some(json!("file_someone_else".replace("_someone_else", "-else").replacen('-', "_", 1).as_str()))))]
    };
    match kind {
        Kind::AssignRole => members(U, true),
        Kind::UnassignRole => members(V, false),
        Kind::CreateRole => vec![
            ("a role of that name that was there before", Box::new(|w: &mut World| w.roles[3]["name"] = json!("Greeters"))),
            (
                "a new role of that name with other rights",
                Box::new(|w: &mut World| {
                    let mut r = role("grol_theirs", "Greeters", 6, &["group-data-manage"], false);
                    r["createdAt"] = json!(now());
                    w.roles.push(r);
                }),
            ),
            (
                "the same role, made before the send",
                Box::new(move |w: &mut World| {
                    let mut r = role("grol_early", "Greeters", 6, &["group-instance-join"], false);
                    r["description"] = json!("Say hello");
                    r["createdAt"] = json!(before);
                    w.roles.push(r);
                }),
            ),
            (
                "the same role, with no time",
                Box::new(|w: &mut World| {
                    let mut r = role("grol_timeless", "Greeters", 6, &["group-instance-join"], false);
                    r["description"] = json!("Say hello");
                    w.roles.push(r);
                }),
            ),
            (
                "two such roles",
                Box::new(|w: &mut World| {
                    for id in ["grol_one", "grol_two"] {
                        let mut r = role(id, "Greeters", 6, &["group-instance-join"], false);
                        r["description"] = json!("Say hello");
                        r["createdAt"] = json!(now());
                        w.roles.push(r);
                    }
                }),
            ),
            ("no list of roles", Box::new(|w: &mut World| w.instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x"}))])),
        ],
        Kind::EditRole => vec![
            ("the role renamed to something else meanwhile", Box::new(|w: &mut World| w.roles[2]["name"] = json!("Sculptors"))),
            ("another role with what was asked", Box::new(|w: &mut World| {
                w.roles[3]["name"] = json!("Painters");
                w.roles[3]["requiresTwoFactor"] = json!(true);
                w.roles[2]["description"] = json!("changed by another");
            })),
            ("the flags left out", Box::new(|w: &mut World| {
                w.roles[2]["name"] = json!("Painters");
                w.roles[2].as_object_mut().unwrap().remove("requiresTwoFactor");
            })),
            ("the role not in the list", Box::new(|w: &mut World| w.roles.retain(|r| r["id"] != "grol_artist"))),
        ],
        Kind::DeleteRole => vec![
            ("no list of roles", Box::new(|w: &mut World| w.instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x"}))])),
            ("an empty list of roles", Box::new(|w: &mut World| w.instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x", "roles": []}))])),
            ("a list cut short", Box::new(|w: &mut World| {
                let many: Vec<Value> = (0..201).map(|i| role(&format!("grol_{i}"), "r", 6, &[], false)).collect();
                w.instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x", "roles": many}))];
            })),
            ("a list with a role that cannot be read", Box::new(|w: &mut World| w.instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x", "roles": [role("grol_mod", "m", 2, &[], false), {"id": 7}]}))])),
        ],
        Kind::CreatePost => vec![
            ("the same announcement, older, that was there before", Box::new(|w: &mut World| {
                w.posts[1] = post("gpos_2", "usr_test", "Saturday", "Bring your art", "group", &["grol_artist"], Value::Null);
                w.posts[1]["createdAt"] = json!(now());
            })),
            ("the same words by someone else", Box::new(|w: &mut World| {
                let mut p = post("gpos_theirs", OTHERS_ID, "Saturday", "Bring your art", "group", &["grol_artist"], Value::Null);
                p["createdAt"] = json!(now());
                w.posts.insert(0, p);
            })),
            ("the same words for everyone", Box::new(|w: &mut World| {
                let mut p = post("gpos_wide", "usr_test", "Saturday", "Bring your art", "group", &[], Value::Null);
                p["createdAt"] = json!(now());
                w.posts.insert(0, p);
            })),
            ("the same words with a picture", Box::new(|w: &mut World| {
                let mut p = post("gpos_pic", "usr_test", "Saturday", "Bring your art", "group", &["grol_artist"], json!("file_x"));
                p["createdAt"] = json!(now());
                w.posts.insert(0, p);
            })),
            ("the same post with its audience left out", Box::new(|w: &mut World| {
                let mut p = post("gpos_part", "usr_test", "Saturday", "Bring your art", "group", &[], Value::Null);
                p["createdAt"] = json!(now());
                p.as_object_mut().unwrap().remove("roleIds");
                w.posts.insert(0, p);
            })),
            ("the same post made before the send", Box::new(move |w: &mut World| {
                let mut p = post("gpos_early", "usr_test", "Saturday", "Bring your art", "group", &["grol_artist"], Value::Null);
                p["createdAt"] = json!(before);
                w.posts.insert(0, p);
            })),
            ("no post at all", Box::new(|w: &mut World| w.posts.clear())),
        ],
        Kind::EditPost => vec![
            ("the title asked, its picture gone", Box::new(|w: &mut World| {
                w.posts[0]["title"] = json!("Someone else's");
                w.posts[0]["imageId"] = Value::Null;
            })),
            ("another post with that title", Box::new(|w: &mut World| {
                w.posts[1]["title"] = json!("Meet-up!");
                w.posts[0]["text"] = json!("changed by another");
            })),
            ("the audience left out", Box::new(|w: &mut World| {
                w.posts[0]["title"] = json!("Meet-up!");
                w.posts[0].as_object_mut().unwrap().remove("roleIds");
            })),
            ("the post out of the list", Box::new(|w: &mut World| {
                w.posts.remove(0);
            })),
        ],
        Kind::DeletePost => vec![
            ("the post out of the list", Box::new(|w: &mut World| {
                w.posts.remove(1);
            })),
            ("an empty list", Box::new(|w: &mut World| w.posts.clear())),
            ("the post unreadable in the list", Box::new(|w: &mut World| w.posts[1]["id"] = json!(5))),
        ],
        Kind::ApplyBanner => art("bannerId"),
        Kind::ApplyIcon => art("iconId"),
        Kind::UploadGallery | Kind::UploadIcon => {
            let tag = if kind == Kind::UploadGallery { "gallery" } else { "icon" };
            let md5 = bytes_md5.unwrap_or_default();
            let with = move |id: &'static str, owner: &'static str, md5: Option<String>| -> Box<dyn Fn(&mut World)> {
                Box::new(move |w: &mut World| {
                    let mut f = file(id, tag, owner, b"other");
                    match &md5 {
                        Some(m) => f["versions"][1]["file"]["md5"] = json!(m),
                        None => {
                            f["versions"][1]["file"].as_object_mut().unwrap().remove("md5");
                        }
                    }
                    w.files.push((tag.into(), f));
                })
            };
            vec![
                ("another picture uploaded meanwhile", with("file_n1", "usr_test", Some("AAAAAAAAAAAAAAAAAAAAAA==".into()))),
                ("the same hash, another size", with("file_n2", "usr_test", Some(md5.clone()))),
                ("the same hash, another owner", with("file_n3", OTHERS_ID, Some(md5))),
                ("no hash", with("file_n4", "usr_test", None)),
                ("nothing new", Box::new(|_: &mut World| {})),
            ]
        }
    }
}
