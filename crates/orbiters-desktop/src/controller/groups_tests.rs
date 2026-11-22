//! Groups against a scripted VRChat (no network, no real group): the outcomes the attack asks to be tested (B1–B4,
//! M1, M3, M6, M7, M9, M10).
use super::*;
use crate::controller::Shared;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_api::{Scripted, Sent};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const G: &str = "grp_00000000-0000-4000-8000-0000000000aa";
const H: &str = "grp_00000000-0000-4000-8000-0000000000ab";
const U: &str = "usr_00000000-0000-4000-8000-0000000000bb";
const TWIN: &str = "usr_00000000-0000-4000-8000-0000000000cc";
const LINK: u64 = 7;

fn ctl(replies: Vec<Scripted>) -> (Shared, Arc<Mutex<Vec<Sent>>>) {
    let c = Controller::new(Settings::default());
    let sent = c.test_vrchat_scripted(replies);
    (c, sent)
}
fn ok(v: Value) -> Scripted {
    Scripted::Answer(200, v)
}
fn roles() -> Value {
    json!([
        {"id": "grol_owner", "name": "Owner", "order": 0, "permissions": ["*"]},
        {"id": "grol_mod", "name": "Moderators", "order": 2, "permissions": ["group-instance-moderate"], "isManagementRole": true},
        {"id": "grol_artist", "name": "Artists", "order": 3, "permissions": []},
        {"id": "grol_everyone", "name": "Everyone", "order": 9, "permissions": ["group-instance-join"], "defaultRole": true}
    ])
}
/// The made-up group as its owner reads it.
fn group(rights: &[&str]) -> Value {
    json!({"id": G, "name": "Made-up Makers", "shortCode": "MAKE", "discriminator": "0001", "ownerId": "usr_test", "memberCount": 120,
        "myMember": {"id": "gmem_me", "userId": "usr_test", "roleIds": ["grol_owner"], "permissions": rights, "membershipStatus": "member"},
        "roles": roles()})
}
fn member(user: &str, roles: &[&str]) -> Value {
    json!({"id": "gmem_x", "userId": user, "roleIds": roles, "user": {"displayName": "Sam"}})
}
fn post() -> Value {
    json!({"id": "gpos_1", "title": "Meet-up", "text": "Saturday", "visibility": "group", "roleIds": [], "imageId": null})
}
fn scope(c: &Shared) -> Scope {
    Scope::of(&c.view())
}
fn calls(sent: &Arc<Mutex<Vec<Sent>>>) -> Vec<String> {
    sent.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.url.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect()
}
fn prepared(reply: Reply) -> Prepared {
    match reply {
        Reply::Prepared(p) => *p,
        other => panic!("{other:?}"),
    }
}
fn draft() -> PostDraft {
    PostDraft { title: "Meet-up".into(), text: "Saturday".into(), ..Default::default() }
}

/// M1: owners and moderators listed (instance moderators included), viewers not; a hidden membership read on its own;
/// keyed by group id; a failed discovery is a failure, never "no groups".
#[test]
fn discovery_lists_managers_and_tells_failure_from_zero() {
    let hidden = "grp_00000000-0000-4000-8000-0000000000ac";
    let (c, sent) = ctl(vec![
        ok(json!({G: ["*"], H: ["group-instance-moderate"], "grp_00000000-0000-4000-8000-0000000000ad": ["group-members-viewall"], hidden: ["group-bans-manage"]})),
        ok(json!([
            {"id": "gmem_1", "groupId": G, "name": "Made-up Makers", "ownerId": "usr_test", "memberCount": 120},
            {"id": "gmem_2", "groupId": H, "name": "Night Painters", "ownerId": "usr_other", "memberCount": 30},
            {"id": "gmem_3", "groupId": "grp_00000000-0000-4000-8000-0000000000ad", "name": "Viewers", "ownerId": "usr_other"}
        ])),
        ok(json!({"id": hidden, "name": "Quiet Club", "ownerId": "usr_other", "memberCount": 4})),
    ]);
    let Reply::Discovered(cards) = c.groups(LINK, scope(&c), Request::Discover) else { panic!() };
    let names: Vec<&str> = cards.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Made-up Makers", "Night Painters", "Quiet Club"]);
    assert!(cards[0].owner && !cards[1].owner);
    assert_eq!(cards[1].id, H, "the group's id, not the membership's");
    assert_eq!(calls(&sent), ["GET /users/usr_test/groups/permissions", "GET /users/usr_test/groups", format!("GET /groups/{hidden}?includeRoles=true").as_str()]);

    let (c, _) = ctl(vec![ok(json!({})), ok(json!([]))]);
    assert_eq!(c.groups(LINK, scope(&c), Request::Discover), Reply::Discovered(vec![]), "zero groups");
    let (c, _) = ctl(vec![Scripted::Answer(503, json!({}))]);
    assert_eq!(c.groups(LINK, scope(&c), Request::Discover), Reply::Failed(Failure::Api(GroupFailure::Unavailable)), "a failure, not zero");
}

/// B3 and r21: a cached account while connecting grants nothing, and nothing is asked.
#[test]
fn connecting_grants_nothing() {
    let (c, sent) = ctl(vec![]);
    c.update(|s| {
        s.vrchat_login.known = s.vrchat_login.user.take();
        s.vrchat_login.restoring = true;
    });
    assert_eq!(c.groups(LINK, scope(&c), Request::Discover), Reply::Failed(Failure::SignedOut));
    assert!(sent.lock().unwrap().is_empty());
}

/// B2: a post whose answer is lost is "unknown"; nothing is retried, a new post waits for a deliberate check; the same
/// prepared post committed twice goes once.
#[test]
fn commit_then_drop_is_unknown_and_holds_until_checked() {
    let (c, sent) = ctl(vec![
        ok(group(&["*"])),     // prepare
        ok(group(&["*"])),     // commit's fresh rules
        Scripted::Drop,        // the post: taken, then the connection died
        ok(group(&["*"])),     // check
        ok(json!([])),         // the check's catalogue (owner: rights over roles)
        ok(group(&["*"])),     // a new prepare after the check
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    assert!(p.confirm, "a post is always confirmed");
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Unknown));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Refused(Failure::Duplicate)), "a duplicate submission is dropped");
    assert_eq!(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }), Reply::Failed(Failure::CheckFirst));
    assert!(matches!(c.groups(LINK, s.clone(), Request::Check { group: G.into() }), Reply::Opened(_)));
    assert!(matches!(c.groups(LINK, s, Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }), Reply::Prepared(_)));
    let posts = calls(&sent).iter().filter(|c| c.starts_with("POST")).count();
    assert_eq!(posts, 1, "sent once: {:?}", calls(&sent));
}

/// B3: A → Connecting → B while a write waits for its confirmation: it can no longer be sent by anyone.
#[test]
fn an_account_switch_drops_a_queued_write() {
    let (c, sent) = ctl(vec![ok(group(&["*"]))]);
    let a = scope(&c);
    let p = prepared(c.groups(LINK, a.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    c.update(|s| {
        s.vrchat_login.known = s.vrchat_login.user.take();
        s.vrchat_login.restoring = true;
        s.vrchat_login.revision += 1;
    });
    assert_eq!(c.groups(LINK, a.clone(), Request::Commit { op: p.op.clone() }), Reply::Failed(Failure::AccountChanged));
    c.update(|s| {
        s.vrchat_login.restoring = false;
        s.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_00000000-0000-4000-8000-0000000000b2".into(), ..Default::default() });
        s.vrchat_login.revision += 1;
    });
    let b = scope(&c);
    assert_ne!(a, b);
    assert_eq!(c.groups(LINK, b, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Expired)), "B cannot send A's");
    assert_eq!(calls(&sent), [format!("GET /groups/{G}?includeRoles=true")], "nothing written");
}

/// B3: the account switches while the post is on its way: no data of A's comes back, the outcome is unknown.
#[test]
fn an_account_switch_after_transmission_is_unknown_and_shows_nothing() {
    let cell: Arc<std::sync::OnceLock<Shared>> = Arc::default();
    let later = cell.clone();
    let switch = Box::new(move || {
        later.get().unwrap().update(|s| {
            s.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_00000000-0000-4000-8000-0000000000b2".into(), ..Default::default() });
            s.vrchat_login.revision += 1;
        })
    });
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(group(&["*"])), Scripted::During(switch, 200, post())]);
    let _ = cell.set(c.clone());
    let a = scope(&c);
    let p = prepared(c.groups(LINK, a.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    assert_eq!(c.groups(LINK, a, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Unknown));
    assert_eq!(calls(&sent).iter().filter(|c| c.starts_with("POST")).count(), 1);
}

/// M3: ids captured at the press; a member who already has the role (the list moved) is refused; two members with the
/// same name are told apart by id; a privilege-bearing role is confirmed, a plain one not; a double commit goes once;
/// a role changed between the confirmation and the send is not sent.
#[test]
fn member_roles_are_guarded() {
    let (c, sent) = ctl(vec![
        ok(group(&["*"])), ok(member(TWIN, &[])),                      // prepare: the twin, by id
        ok(group(&["*"])), ok(member(TWIN, &[])), ok(json!(["grol_mod"])), // commit
        ok(group(&["*"])), ok(member(U, &["grol_artist"])),            // already has it: the hit target moved
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: TWIN.into(), role: "grol_mod".into(), add: true } }));
    assert!(p.confirm, "a management role is confirmed");
    let done = c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() });
    assert_eq!(done, Reply::Outcome(Outcome::Done(Done::MemberRoles { user: TWIN.into(), role_ids: vec!["grol_mod".into()] })), "the returned ids are the committed state");
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Duplicate)), "double activation");
    assert_eq!(
        c.groups(LINK, s, Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }),
        Reply::Failed(Failure::Changed)
    );
    let writes: Vec<String> = calls(&sent).into_iter().filter(|c| c.starts_with("PUT") || c.starts_with("DELETE")).collect();
    assert_eq!(writes, [format!("PUT /groups/{G}/members/{TWIN}/roles/grol_mod")], "the twin's id, once");

    // A cosmetic role needs no confirmation; one redefined since the press is not sent.
    let mut changed = roles();
    changed[2]["permissions"] = json!(["group-instance-moderate"]);
    let mut redefined = group(&["*"]);
    redefined["roles"] = changed;
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(member(U, &[])), ok(redefined), ok(member(U, &[]))]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }));
    assert!(!p.confirm, "a role with no rights is no privilege");
    assert_eq!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Changed)));
    assert!(calls(&sent).iter().all(|c| c.starts_with("GET")), "{:?}", calls(&sent));
}

/// M2 through the service: a moderator without the rights cannot prepare a role grant; nothing is written.
#[test]
fn rights_are_checked_on_fresh_data() {
    let mut weak = group(&["group-instance-moderate"]);
    weak["myMember"]["roleIds"] = json!(["grol_mod"]);
    let (c, sent) = ctl(vec![ok(weak), ok(member(U, &[]))]);
    assert_eq!(
        c.groups(LINK, scope(&c), Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }),
        Reply::Failed(Failure::Withheld(Why::NoPermission))
    );
    assert!(calls(&sent).iter().all(|c| c.starts_with("GET")));
}

/// M7: what the confirmation shows is what goes: visibility, roles, the notification (off unless chosen); an edit
/// never notifies.
#[test]
fn post_confirmations_show_what_is_sent_and_edits_never_notify() {
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(group(&["*"])), ok(group(&["*"])), ok(post())]);
    let s = scope(&c);
    let asked = PostDraft { role_ids: vec!["grol_artist".into(), "not-a-role".into()], ..draft() };
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: asked } }));
    let Write::CreatePost { draft: shown, .. } = &p.write else { panic!() };
    assert_eq!(shown.role_ids, ["grol_artist"], "only real role ids");
    assert!(!shown.send_notification, "off by default");
    assert_eq!(shown.visibility, g::Visibility::Group);
    let baseline = orbiters_core::groups::Post { id: "gpos_1".into(), title: "Meet-up".into(), text: "Saturday".into(), ..Default::default() };
    let edit = PostDraft { send_notification: true, text: "Sunday".into(), ..draft() };
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::EditPost { group: G.into(), baseline, edit } }));
    let Write::EditPost { edit: shown, .. } = &p.write else { panic!() };
    assert!(!shown.send_notification, "an edit never notifies");
    assert!(matches!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Done(Done::Post(_)))));
    let sent = sent.lock().unwrap();
    let body: Value = serde_json::from_str(sent.last().unwrap().body.as_deref().unwrap()).unwrap();
    assert!(body.get("sendNotification").is_none(), "{body}");
}

/// M6: an upload is its own step; the apply that follows can fail, and the file id is kept to apply again later
/// without uploading again.
#[test]
fn upload_then_apply_fails_keeps_the_file() {
    let file = json!({"id": "file_new", "versions": [{"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_new/1/file"}}]});
    let (c, sent) = ctl(vec![
        ok(file),
        ok(group(&["*"])), ok(group(&["*"])), Scripted::Answer(403, json!({})), // apply refused
        ok(group(&["*"])), ok(group(&["*"])), ok(group(&["*"])),                // apply again: done
    ]);
    let s = scope(&c);
    let png = b"\x89PNG\r\n\x1a\nmade-up".to_vec();
    let up = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, png } }));
    let Reply::Outcome(Outcome::Done(Done::Uploaded(f))) = c.groups(LINK, s.clone(), Request::Commit { op: up.op }) else { panic!() };
    let apply = || Request::Prepare { write: Write::ApplyArt { group: G.into(), slot: ArtSlot::Banner, file: f.id.clone() } };
    let p = prepared(c.groups(LINK, s.clone(), apply()));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Api(GroupFailure::Forbidden))), "your role does not allow this, not 'not found'");
    let p = prepared(c.groups(LINK, s.clone(), apply()));
    assert!(matches!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Done(Done::Group(_)))));
    let writes: Vec<String> = calls(&sent).into_iter().filter(|c| !c.starts_with("GET")).collect();
    assert_eq!(writes, ["POST /file/image".to_string(), format!("PUT /groups/{G}"), format!("PUT /groups/{G}")], "uploaded once");
    let bodies: Vec<String> = sent.lock().unwrap().iter().filter(|s| s.method == "PUT").map(|s| s.body.clone().unwrap()).collect();
    assert!(bodies.iter().all(|b| b == r#"{"bannerId":"file_new"}"#), "{bodies:?}");
}

/// M10: only the window, and only the link that prepared it, commits a write; the service raises no notice with its
/// text.
#[test]
fn only_the_preparing_window_commits() {
    for role in [crate::ipc::Role::Overlay, crate::ipc::Role::Orbit] {
        assert!(!allowed_from(role), "{role:?}");
    }
    assert!(allowed_from(crate::ipc::Role::Window));
    let (c, sent) = ctl(vec![ok(group(&["*"]))]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::DeleteRole { group: G.into(), role: "grol_artist".into() } }));
    assert!(p.confirm, "a deletion is confirmed");
    assert_eq!(c.groups(LINK + 1, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Refused(Failure::Expired)), "another link cannot commit it");
    assert_eq!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Expired)), "gone once another link tried it");
    assert!(calls(&sent).iter().all(|c| c.starts_with("GET")));
    assert!(c.take_notices().is_empty(), "no notice carries a group's text");
}

/// M9: nothing of a group reaches the shared snapshot, and replies cross the link as real bytes, collections full.
#[test]
fn group_data_stays_out_of_the_snapshot_and_survives_the_link() {
    let (c, _) = ctl(vec![ok(json!({G: ["*"]})), ok(json!([{"id": "gmem_1", "groupId": G, "name": "Secret Society Of Made-up Names", "ownerId": "usr_test"}]))]);
    let reply = c.groups(LINK, scope(&c), Request::Discover);
    let Reply::Discovered(cards) = &reply else { panic!() };
    assert_eq!(cards.len(), 1);
    let snapshot = serde_json::to_string(&*c.view()).unwrap();
    assert!(!snapshot.contains("Secret Society"), "the snapshot every subscriber gets holds no group");
    let bytes = serde_json::to_vec(&crate::ipc::ToClient::Groups { id: "1".into(), reply: reply.clone() }).unwrap();
    let back: crate::ipc::ToClient = serde_json::from_slice(&bytes).unwrap();
    let crate::ipc::ToClient::Groups { reply: back, .. } = back else { panic!() };
    assert_eq!(back, reply);
    let png = vec![0x89, b'P', b'N', b'G', 0, 1, 2, 255];
    let ask = crate::ipc::ToService::Groups { id: "2".into(), scope: scope(&c), request: Request::Prepare { write: Write::Upload { tag: FileTag::Icon, png: png.clone() } } };
    let bytes = serde_json::to_vec(&ask).unwrap();
    assert!(bytes.len() < 400, "pictures cross as base64, not number lists");
    let crate::ipc::ToService::Groups { request: Request::Prepare { write: Write::Upload { png: back, .. } }, .. } = serde_json::from_slice(&bytes).unwrap() else { panic!() };
    assert_eq!(back, png);
}

/// M9: requests are bounded: past the few running at once, the next is "busy" without asking VRChat.
#[test]
fn requests_are_bounded() {
    let (c, sent) = ctl(vec![]);
    c.groups.lock().unwrap().running = RUNNING_MAX;
    assert_eq!(c.groups(LINK, scope(&c), Request::Discover), Reply::Failed(Failure::Busy));
    assert!(sent.lock().unwrap().is_empty());
}
