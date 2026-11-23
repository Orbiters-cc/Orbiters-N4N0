//! Groups against a scripted VRChat (no network, no real group): the outcomes the attack and the verification ask to
//! be tested (B1–B4, M1–M12 of the attack; Blockers, Majors and Minors of verify-r22-groups.md).
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

pub(crate) fn ctl(replies: Vec<Scripted>) -> (Shared, Arc<Mutex<Vec<Sent>>>) {
    let c = Controller::new(Settings::default());
    let sent = c.test_vrchat_scripted(replies);
    (c, sent)
}
pub(crate) fn ok(v: Value) -> Scripted {
    Scripted::Answer(200, v)
}
/// A role as VRChat describes it whole (every field the rules decide by).
fn role(id: &str, name: &str, order: i64, rights: &[&str], management: bool) -> Value {
    json!({"id": id, "name": name, "order": order, "permissions": rights, "defaultRole": false, "isManagementRole": management, "requiresPurchase": false})
}
pub(crate) fn roles() -> Value {
    let mut everyone = role("grol_everyone", "Everyone", 9, &["group-instance-join"], false);
    everyone["defaultRole"] = json!(true);
    json!([
        role("grol_owner", "Owner", 0, &["*"], true),
        role("grol_mod", "Moderators", 2, &["group-instance-moderate"], true),
        role("grol_artist", "Artists", 3, &[], false),
        everyone
    ])
}
/// The made-up group as its owner reads it.
pub(crate) fn group(rights: &[&str]) -> Value {
    json!({"id": G, "name": "Made-up Makers", "shortCode": "MAKE", "discriminator": "0001", "ownerId": "usr_test", "memberCount": 120,
        "myMember": {"id": "gmem_me", "userId": "usr_test", "roleIds": ["grol_owner"], "permissions": rights, "membershipStatus": "member"},
        "roles": roles()})
}
fn member(user: &str, roles: &[&str]) -> Value {
    json!({"id": "gmem_x", "groupId": G, "userId": user, "roleIds": roles, "user": {"displayName": "Sam"}})
}
pub(crate) fn post_json(title: &str, visibility: &str, roles: &[&str]) -> Value {
    json!({"id": "gpos_1", "groupId": G, "authorId": "usr_test", "title": title, "text": "Saturday", "visibility": visibility, "roleIds": roles, "imageId": null})
}
fn posts_page(items: Vec<Value>) -> Value {
    let n = items.len();
    json!({"posts": items, "total": n})
}
pub(crate) fn scope(c: &Shared) -> Scope {
    Scope::of(&c.view())
}
fn calls(sent: &Arc<Mutex<Vec<Sent>>>) -> Vec<String> {
    sent.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.url.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect()
}
fn writes(sent: &Arc<Mutex<Vec<Sent>>>) -> Vec<String> {
    calls(sent).into_iter().filter(|c| !c.starts_with("GET")).collect()
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
fn the_post(c: Value) -> Post {
    orbiters_core::groups::Post { id: "gpos_1".into(), title: c["title"].as_str().unwrap().into(), text: "Saturday".into(), visibility: if c["visibility"] == "public" { g::Visibility::Public } else { g::Visibility::Group }, role_ids: c["roleIds"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect(), complete: true, author_id: "usr_test".into(), ..Default::default() }
}

/// M1 and Major 7: owners and moderators listed (instance moderators included), viewers not; every hidden membership
/// listed, the first read at once, the others named when their cards are asked; keyed by group id; a failure is a
/// failure, never "no groups".
#[test]
fn discovery_lists_every_qualifying_group_and_tells_failure_from_zero() {
    let hidden: Vec<String> = (0..20).map(|i| format!("grp_00000000-0000-4000-8000-0000000001{i:02}")).collect();
    let mut rights = serde_json::Map::new();
    rights.insert(G.into(), json!(["*"]));
    rights.insert(H.into(), json!(["group-instance-moderate"]));
    rights.insert("grp_00000000-0000-4000-8000-0000000000ad".into(), json!(["group-members-viewall"]));
    for h in &hidden {
        rights.insert(h.clone(), json!(["group-bans-manage"]));
    }
    let mut replies = vec![
        ok(Value::Object(rights)),
        ok(json!([
            {"id": "gmem_1", "groupId": G, "name": "Made-up Makers", "ownerId": "usr_test", "memberCount": 120},
            {"id": "gmem_2", "groupId": H, "name": "Night Painters", "ownerId": "usr_other", "memberCount": 30},
            {"id": "gmem_3", "groupId": "grp_00000000-0000-4000-8000-0000000000ad", "name": "Viewers", "ownerId": "usr_other"}
        ])),
    ];
    // The first twelve hidden ones read at discovery (in the rights' order, a BTreeMap's).
    let mut sorted = hidden.clone();
    sorted.sort();
    for h in sorted.iter().take(12) {
        replies.push(ok(json!({"id": h, "name": format!("Hidden {}", &h[h.len() - 2..]), "ownerId": "usr_other", "memberCount": 4})));
    }
    // Then the window asks for the eight left, as their cards come into view.
    for h in sorted.iter().skip(12) {
        replies.push(ok(json!({"id": h, "name": format!("Hidden {}", &h[h.len() - 2..]), "ownerId": "usr_other", "memberCount": 0, "myMember": {"userId": "usr_test", "roleIds": [], "permissions": ["group-bans-manage"], "membershipStatus": "member"}})));
    }
    let (c, _) = ctl(replies);
    let Reply::Discovered(cards) = c.groups(LINK, scope(&c), Request::Discover) else { panic!() };
    assert_eq!(cards.len(), 22, "every qualifying group, the eight unnamed included");
    assert!(cards[0].owner && cards[0].name == "Made-up Makers");
    assert!(cards.iter().any(|c| c.id == H && !c.owner), "an instance moderator's group");
    assert!(!cards.iter().any(|c| c.name == "Viewers"), "view-all alone is not listed");
    let unnamed: Vec<String> = cards.iter().filter(|c| !c.named).map(|c| c.id.clone()).collect();
    assert_eq!(unnamed.len(), 8);
    assert!(cards.iter().filter(|c| !c.named).all(|c| c.member_count.is_none()), "a count not read is not zero");
    let Reply::Hydrated(named) = c.groups(LINK, scope(&c), Request::Hydrate { ids: unnamed.clone() }) else { panic!() };
    assert_eq!(named.len(), 8);
    assert!(named.iter().all(|c| c.named && c.member_count == Some(0) && c.name.starts_with("Hidden")));

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

/// B2 (attack) and Blocker 2: a post whose answer is lost is "unknown"; nothing is retried; a new post is answered with
/// the held write; a check that reads the posts and finds it settles it as done; a duplicate commit is dropped.
#[test]
fn commit_then_drop_is_unknown_and_a_check_reads_the_posts() {
    let (c, sent) = ctl(vec![
        ok(group(&["*"])), // prepare
        ok(group(&["*"])), // commit's fresh rules
        Scripted::Drop,    // the post: taken, then the connection died
        ok(posts_page(vec![post_json("Meet-up", "group", &[])])), // the check reads the posts: it is there
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    assert!(p.confirm, "a post is always confirmed");
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Unknown));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Refused(Failure::Duplicate)), "a duplicate submission is dropped");
    let held = c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } });
    assert!(matches!(&held, Reply::Held { op, .. } if *op == p.op), "the window is shown which write to check: {held:?}");
    let checked = c.groups(LINK, s.clone(), Request::Check { op: p.op.clone(), write: p.write.clone() });
    assert_eq!(checked, Reply::Checked { op: p.op.clone(), observation: Observation::Applied }, "the post is seen on VRChat");
    assert_eq!(writes(&sent).len(), 1, "sent once: {:?}", calls(&sent));
    assert!(calls(&sent).last().unwrap().starts_with(&format!("GET /groups/{G}/posts")), "the check read the posts, not just the group");
}

/// Blocker 2: a check that cannot see the post among every post settles nothing: the hold stays until the person lets
/// it go; a check that saw every post and not it says "not done"; two uncertain writes on different targets are checked
/// apart.
#[test]
fn an_inconclusive_check_holds_until_the_person_lets_go() {
    let full: Vec<Value> = (0..50).map(|i| json!({"id": format!("gpos_{i}"), "groupId": G, "authorId": "usr_other", "title": "Other", "text": "x", "visibility": "group"})).collect();
    let (c, sent) = ctl(vec![
        ok(group(&["*"])), ok(group(&["*"])), Scripted::Drop, // the post: unknown
        ok(group(&["*"])), ok(member(U, &[])), ok(group(&["*"])), ok(member(U, &[])), Scripted::Drop, // a role given: unknown
        ok(json!({"posts": full.clone(), "total": 500})), ok(json!({"posts": full.clone(), "total": 500})), ok(json!({"posts": full.clone(), "total": 500})), ok(json!({"posts": full, "total": 500})), // 4 pages, not all seen
        ok(member(U, &[])), // the role check: the member does not have it: not done
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() }), Reply::Outcome(Outcome::Unknown));
    let r = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: r.op.clone() }), Reply::Outcome(Outcome::Unknown));
    assert_eq!(c.groups(LINK, s.clone(), Request::Check { op: p.op.clone(), write: p.write.clone() }), Reply::Checked { op: p.op.clone(), observation: Observation::Inconclusive });
    assert!(matches!(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }), Reply::Held { .. }), "inconclusive keeps the hold");
    assert_eq!(c.groups(LINK, s.clone(), Request::Release { op: r.op.clone() }), Reply::Failed(Failure::CheckFirst), "the role's hold is not let go unchecked");
    assert_eq!(c.groups(LINK, s.clone(), Request::Check { op: r.op.clone(), write: r.write.clone() }), Reply::Checked { op: r.op.clone(), observation: Observation::NotApplied }, "the role's own check");
    assert_eq!(c.groups(LINK, s.clone(), Request::Release { op: p.op.clone() }), Reply::Released, "after an inconclusive check, the person decides");
    assert_eq!(writes(&sent).len(), 2);
}

/// Blocker 1: the commit's answer is lost on the link (the window sees nothing); the service kept the outcome by op, so
/// the window's check gets "done", not a refusal — and a commit that never arrived is "not sent".
#[test]
fn a_lost_reply_is_recovered_by_op() {
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(group(&["*"])), ok(post_json("Meet-up", "group", &[])), ok(group(&["*"]))]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    let _lost = c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() });
    let Reply::Checked { observation: Observation::Recorded(Outcome::Done(Done::Post(post))), .. } = c.groups(LINK, s.clone(), Request::Check { op: p.op.clone(), write: p.write.clone() }) else { panic!() };
    assert_eq!(post.id, "gpos_1");
    let q = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: PostDraft { title: "Again".into(), ..draft() } } }));
    assert_eq!(c.groups(LINK, s, Request::Check { op: q.op.clone(), write: q.write }), Reply::Checked { op: q.op, observation: Observation::NotSent });
    assert_eq!(writes(&sent).len(), 1);
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
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(group(&["*"])), Scripted::During(switch, 200, post_json("Meet-up", "group", &[]))]);
    let _ = cell.set(c.clone());
    let a = scope(&c);
    let p = prepared(c.groups(LINK, a.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: draft() } }));
    assert_eq!(c.groups(LINK, a, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Unknown));
    assert_eq!(writes(&sent).len(), 1);
}

/// M3: ids captured at the press; a member who already has the role (the list moved) is refused; two members with the
/// same name are told apart by id; a privilege-bearing role is confirmed, a plain one not; a double commit goes once;
/// a role changed between the confirmation and the send is not sent.
#[test]
fn member_roles_are_guarded() {
    let (c, sent) = ctl(vec![
        ok(group(&["*"])), ok(member(TWIN, &[])),
        ok(group(&["*"])), ok(member(TWIN, &[])), ok(json!(["grol_mod"])),
        ok(group(&["*"])), ok(member(U, &["grol_artist"])),
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: TWIN.into(), role: "grol_mod".into(), add: true } }));
    assert!(p.confirm, "a management role is confirmed");
    assert_eq!(p.member.as_ref().map(|m| m.membership_id.as_str()), Some("gmem_x"), "the confirmation carries ids that tell namesakes apart");
    let done = c.groups(LINK, s.clone(), Request::Commit { op: p.op.clone() });
    assert_eq!(done, Reply::Outcome(Outcome::Done(Done::MemberRoles { user: TWIN.into(), role_ids: vec!["grol_mod".into()] })));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Duplicate)), "double activation");
    assert_eq!(c.groups(LINK, s, Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }), Reply::Failed(Failure::Changed));
    assert_eq!(writes(&sent), [format!("PUT /groups/{G}/members/{TWIN}/roles/grol_mod")]);

    let mut changed = roles();
    changed[2]["permissions"] = json!(["group-instance-moderate"]);
    let mut redefined = group(&["*"]);
    redefined["roles"] = changed;
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(member(U, &[])), ok(redefined), ok(member(U, &[]))]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }));
    assert!(!p.confirm, "a role known to hold no rights is no privilege");
    assert_eq!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Changed)));
    assert!(writes(&sent).is_empty());
}

/// Blocker 4: a role read without its rights or flags is not given in one click (nor at all), and a member read
/// without role arrays is not changed.
#[test]
fn incomplete_role_or_member_data_withholds_the_assignment() {
    let mut bare = roles();
    bare[2] = json!({"id": "grol_artist", "name": "Artists", "order": 3});
    let mut g_bare = group(&["*"]);
    g_bare["roles"] = bare;
    let no_roles = json!({"id": "gmem_x", "groupId": G, "userId": U, "user": {"displayName": "Sam"}});
    let (c, sent) = ctl(vec![ok(g_bare), ok(member(U, &[])), ok(group(&["*"])), ok(no_roles)]);
    let s = scope(&c);
    assert_eq!(
        c.groups(LINK, s.clone(), Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }),
        Reply::Failed(Failure::Withheld(Why::MissingData)),
        "no permission list: never a silent cosmetic grant"
    );
    assert_eq!(
        c.groups(LINK, s, Request::Prepare { write: Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true } }),
        Reply::Failed(Failure::Withheld(Why::MissingData)),
        "no role arrays: their roles are unknown"
    );
    assert!(writes(&sent).is_empty());
}

/// Blocker 7: a role confirmed for deletion that another manager renamed and gave rights before the send is not
/// deleted; a post confirmed for deletion that changed is not deleted; a post not found is not deleted blind.
#[test]
fn a_deletion_whose_subject_changed_is_refused() {
    let mut grown = roles();
    grown[2] = role("grol_artist", "Senior staff", 3, &["group-instance-moderate"], true);
    let mut g_grown = group(&["*"]);
    g_grown["roles"] = grown;
    let (c, sent) = ctl(vec![
        ok(group(&["*"])), // prepare: Artists, no rights
        ok(g_grown),       // commit: renamed, rights given meanwhile
        ok(group(&["*"])), ok(posts_page(vec![post_json("Meet-up", "group", &[])])), // prepare the post's deletion
        ok(group(&["*"])), ok(posts_page(vec![post_json("Meet-up, edited", "public", &[])])), // commit: it changed
        ok(group(&["*"])), ok(posts_page(vec![])), // a post not found
    ]);
    let s = scope(&c);
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::DeleteRole { group: G.into(), role: "grol_artist".into() } }));
    assert_eq!(p.role.as_ref().map(|r| r.name.as_str()), Some("Artists"), "the confirmation shows the role as read");
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Changed)));
    let shown = the_post(post_json("Meet-up", "group", &[]));
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::DeletePost { group: G.into(), post: shown.clone() } }));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Changed)));
    assert_eq!(c.groups(LINK, s, Request::Prepare { write: Write::DeletePost { group: G.into(), post: shown } }), Reply::Failed(Failure::Withheld(Why::MissingData)));
    assert!(writes(&sent).is_empty(), "{:?}", calls(&sent));
}

/// Blocker 3: two editors. A opened a public post; B restricted it to a role; A's typo fix does not widen it back.
/// An audience this version cannot carry whole is refused, never narrowed or widened quietly.
#[test]
fn a_post_edit_never_widens_an_audience_changed_meanwhile() {
    let seen_by_a = the_post(post_json("Meet-up", "public", &[]));
    let now = post_json("Meet-up", "group", &["grol_artist"]);
    // Another post: its picture removed by B since A saw it, its words and audience as they were.
    let with_picture = Post { image_id: Some("file_00000000-0000-4000-8000-0000000000f3".into()), ..seen_by_a.clone() };
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(posts_page(vec![now])), ok(group(&["*"])), ok(posts_page(vec![post_json("Meet-up", "public", &[])])), ok(group(&["*"]))]);
    let s = scope(&c);
    let edit = PostDraft { title: "Meet-up!".into(), text: "Saturday".into(), visibility: g::Visibility::Public, ..Default::default() };
    assert_eq!(c.groups(LINK, s.clone(), Request::Prepare { write: Write::EditPost { group: G.into(), baseline: seen_by_a, edit } }), Reply::Failed(Failure::Changed));
    let kept = PostDraft { title: "Meet-up!".into(), text: "Saturday".into(), visibility: g::Visibility::Public, image_id: with_picture.image_id.clone(), ..Default::default() };
    assert_eq!(c.groups(LINK, s.clone(), Request::Prepare { write: Write::EditPost { group: G.into(), baseline: with_picture, edit: kept } }), Reply::Failed(Failure::Changed), "a removed picture is not put back");
    let odd = PostDraft { role_ids: vec!["grol_artist".into(), "not a role".into()], ..draft() };
    assert_eq!(c.groups(LINK, s, Request::Prepare { write: Write::CreatePost { group: G.into(), draft: odd } }), Reply::Failed(Failure::Api(GroupFailure::Invalid)), "an id that is not a role is refused, not dropped");
    assert!(writes(&sent).is_empty());
}

/// M7: what the confirmation shows is what goes: visibility, roles, the notification (off unless chosen); an edit
/// never notifies; the edit is checked against the post as VRChat has it now and keeps its audience.
#[test]
fn post_confirmations_show_what_is_sent_and_edits_never_notify() {
    let current = post_json("Meet-up", "group", &["grol_artist"]);
    let edited = json!({"id": "gpos_1", "groupId": G, "title": "Meet-up", "text": "Sunday", "visibility": "group", "roleIds": ["grol_artist"]});
    let (c, sent) = ctl(vec![ok(group(&["*"])), ok(group(&["*"])), ok(posts_page(vec![current.clone()])), ok(group(&["*"])), ok(posts_page(vec![current.clone()])), ok(edited)]);
    let s = scope(&c);
    let asked = PostDraft { role_ids: vec!["grol_artist".into()], ..draft() };
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::CreatePost { group: G.into(), draft: asked } }));
    let Write::CreatePost { draft: shown, .. } = &p.write else { panic!() };
    assert_eq!(shown.role_ids, ["grol_artist"]);
    assert!(!shown.send_notification, "off by default");
    let baseline = the_post(current);
    let edit = PostDraft { send_notification: true, text: "Sunday".into(), role_ids: baseline.role_ids.clone(), ..draft() };
    let p = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::EditPost { group: G.into(), baseline, edit } }));
    let Write::EditPost { edit: shown, .. } = &p.write else { panic!() };
    assert!(!shown.send_notification, "an edit never notifies");
    assert!(matches!(c.groups(LINK, s, Request::Commit { op: p.op }), Reply::Outcome(Outcome::Done(Done::Post(_)))));
    let sent = sent.lock().unwrap();
    let body: Value = serde_json::from_str(sent.last().unwrap().body.as_deref().unwrap()).unwrap();
    assert!(body.get("sendNotification").is_none(), "{body}");
    assert_eq!(body["roleIds"], json!(["grol_artist"]), "the restriction kept");
}

/// M6 and Major 1, Major 4: an upload is its own step, its PNG staged on disk (never on the link), only for an account
/// that may upload; the apply that follows can fail, and the file id is kept to apply again later without uploading
/// again; a file that is not the account's own is never applied.
#[test]
fn upload_then_apply_with_eligibility_and_ownership() {
    let file = json!({"id": "file_00000000-0000-4000-8000-0000000000f1", "versions": [{"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-0000000000f1/1/file"}}]});
    let me = json!({"id": "usr_test", "tags": ["system_supporter"]});
    let inventory = json!([file.clone()]);
    let (c, sent) = ctl(vec![
        ok(me.clone()),                                                                           // prepare upload: VRChat+
        ok(me),                                                                                   // commit re-check
        ok(file),
        ok(group(&["*"])), ok(inventory.clone()), ok(group(&["*"])), ok(inventory.clone()), Scripted::Answer(403, json!({})), // apply refused
        ok(group(&["*"])), ok(inventory.clone()), ok(group(&["*"])), ok(inventory.clone()), ok(json!({"id": G, "bannerId": "file_00000000-0000-4000-8000-0000000000f1"})), // again: done
        ok(group(&["*"])), ok(inventory),                                                         // a foreign file
    ]);
    let s = scope(&c);
    let png = b"\x89PNG\r\n\x1a\nmade-up".to_vec();
    let staged = crate::pages::groups::stage_upload(&png).unwrap();
    let up = prepared(c.groups(LINK, s.clone(), Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, group: G.into(), staged: staged.clone() } }));
    let Reply::Outcome(Outcome::Done(Done::Uploaded(f))) = c.groups(LINK, s.clone(), Request::Commit { op: up.op }) else { panic!() };
    assert!(!staging_dir().join(&staged.name).exists(), "the staged file goes once sent");
    let apply = |file: &str| Request::Prepare { write: Write::ApplyArt { group: G.into(), slot: ArtSlot::Banner, file: file.into() } };
    let p = prepared(c.groups(LINK, s.clone(), apply(&f.id)));
    assert_eq!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Refused(Failure::Api(GroupFailure::Forbidden))));
    let p = prepared(c.groups(LINK, s.clone(), apply(&f.id)));
    assert!(matches!(c.groups(LINK, s.clone(), Request::Commit { op: p.op }), Reply::Outcome(Outcome::Done(Done::Group(_)))));
    assert_eq!(c.groups(LINK, s, apply("file_00000000-0000-4000-8000-0000000000f2")), Reply::Failed(Failure::Withheld(Why::NotYourPicture)));
    assert_eq!(writes(&sent), ["POST /file/image".to_string(), format!("PUT /groups/{G}"), format!("PUT /groups/{G}")], "uploaded once");
    let sent = sent.lock().unwrap();
    assert!(sent.iter().filter(|s| s.method == "PUT").all(|s| s.body.as_deref() == Some(r#"{"bannerId":"file_00000000-0000-4000-8000-0000000000f1"}"#)));
}

/// Major 4: an account without VRChat+ is told so before anything is sent.
#[test]
fn an_upload_needs_vrchat_plus() {
    let (c, sent) = ctl(vec![ok(json!({"id": "usr_test", "tags": []}))]);
    let staged = crate::pages::groups::stage_upload(b"\x89PNG\r\n\x1a\nx").unwrap();
    assert_eq!(c.groups(LINK, scope(&c), Request::Prepare { write: Write::Upload { tag: FileTag::Icon, group: G.into(), staged } }), Reply::Failed(Failure::Withheld(Why::NotEligible)));
    assert!(writes(&sent).is_empty());
}

/// Major 1: a staged file that is not what was announced (another size or hash) is never sent.
#[test]
fn a_staged_upload_must_be_the_one_announced() {
    let (c, sent) = ctl(vec![]);
    let mut staged = crate::pages::groups::stage_upload(b"\x89PNG\r\n\x1a\nx").unwrap();
    staged.sha256 = "0".repeat(64);
    assert_eq!(c.groups(LINK, scope(&c), Request::Prepare { write: Write::Upload { tag: FileTag::Icon, group: G.into(), staged: staged.clone() } }), Reply::Failed(Failure::Changed));
    staged.name = "../../secret.png".into();
    assert_eq!(c.groups(LINK, scope(&c), Request::Prepare { write: Write::Upload { tag: FileTag::Icon, group: G.into(), staged } }), Reply::Failed(Failure::Api(GroupFailure::Invalid)));
    assert!(sent.lock().unwrap().is_empty());
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
    assert!(writes(&sent).is_empty());
}

/// M10: only the window, and only the link that prepared it, commits a write; the service raises no notice.
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
    assert!(writes(&sent).is_empty());
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
}

/// M9: the service admits a few groups requests at once, before their threads start; reads are bounded inside too.
#[test]
fn requests_are_bounded() {
    let (c, sent) = ctl(vec![]);
    let held: Vec<Admitted> = (0..ADMIT_MAX).map(|_| c.groups_admit().expect("admitted")).collect();
    assert!(c.groups_admit().is_none(), "the next waits: nothing started for it");
    drop(held);
    assert!(c.groups_admit().is_some(), "taken in again once one ends");
    c.groups.lock().unwrap().running = RUNNING_MAX;
    assert_eq!(c.groups(LINK, scope(&c), Request::Discover), Reply::Failed(Failure::Busy));
    assert!(sent.lock().unwrap().is_empty());
}
