//! Groups' service against a made-up VRChat (no network, no real group): the attack's, the two verifications' and the
//! design's outcomes. The window's side, and the matrix of every write and every failure, are in `pages/groups`.
use super::world::{decoys, role, route, scope, world, write_of, writes, Fault, Hold, Shown, World, G, OTHERS_WORDS, TWIN, U, V};
use super::*;
use crate::controller::Shared;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_api::{Scripted, Sent};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const H: &str = "grp_00000000-0000-4000-8000-0000000000ab";
const LINK: u64 = 7;

fn ctl(replies: Vec<Scripted>) -> (Shared, Arc<Mutex<Vec<Sent>>>) {
    let c = Controller::new(Settings::default());
    let sent = c.test_vrchat_scripted(replies);
    (c, sent)
}
fn ok(v: Value) -> Scripted {
    Scripted::Answer(200, v)
}
fn ask(c: &Shared, r: Request) -> Reply {
    c.groups(LINK, scope(c), r)
}
fn the(c: &Shared, kind: Kind) -> Write {
    write_of(kind, &|r| ask(c, r))
}
fn prepare(c: &Shared, write: Write) -> Result<Sealed, Reply> {
    match ask(c, Request::Prepare { write }) {
        Reply::Prepared(p) => Ok(*p),
        other => Err(other),
    }
}
fn commit(c: &Shared, s: &Sealed) -> Outcome {
    match ask(c, Request::Commit { op: s.op.clone(), digest: s.digest() }) {
        Reply::Outcome { outcome, .. } => outcome,
        other => panic!("{other:?}"),
    }
}
fn failed(f: Failure) -> Result<Sealed, Reply> {
    Err(Reply::Failed(f))
}
fn entry(c: &Shared, op: &str) -> Entry {
    c.groups_book().get(&Key::of(&scope(c)).unwrap(), op).unwrap()
}
fn check(c: &Shared, op: &str) -> Observation {
    match ask(c, Request::Check { op: op.into() }) {
        Reply::Checked { observation, .. } => observation,
        other => panic!("{other:?}"),
    }
}
fn later(c: &Shared) {
    c.groups_book().ahead_ms.store(super::super::groups_ledger::SETTLES_AFTER_MS + 1000, std::sync::atomic::Ordering::Relaxed);
}
/// The write sent, carried out or not, its answer lost.
fn lost(c: &Shared, w: &Shown, kind: Kind, done: bool) -> String {
    let s = prepare(c, the(c, kind)).unwrap();
    w.lock().unwrap().fault = Some(Fault::Drop { done });
    assert_eq!(commit(c, &s), Outcome::Unknown, "{kind:?}");
    s.op
}

/// M1 and Major 7: owners and moderators listed (instance moderators included), viewers not; every hidden membership
/// listed, the first read at once, the others named when their cards are asked; keyed by group id; a failure is a
/// failure, never "no groups". J8: one card that fails leaves the others. N1: a count not given is not zero.
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
    let mut sorted = hidden.clone();
    sorted.sort();
    for h in sorted.iter().take(12) {
        replies.push(ok(json!({"id": h, "name": format!("Hidden {}", &h[h.len() - 2..]), "ownerId": "usr_other", "memberCount": 4})));
    }
    // The eight left, as their cards come into view: the third cannot be read, the fourth has no count.
    for (i, h) in sorted.iter().skip(12).enumerate() {
        let mut v = json!({"id": h, "name": format!("Hidden {}", &h[h.len() - 2..]), "ownerId": "usr_other", "memberCount": 0});
        if i == 3 {
            v.as_object_mut().unwrap().remove("memberCount");
        }
        replies.push(if i == 2 { Scripted::Answer(404, json!({})) } else { ok(v) });
    }
    let (c, _) = ctl(replies);
    let Reply::Discovered(cards) = ask(&c, Request::Discover) else { panic!() };
    assert_eq!(cards.len(), 22, "every qualifying group, the eight unnamed included");
    assert!(cards[0].owner && cards[0].name == "Made-up Makers");
    assert!(cards.iter().any(|c| c.id == H && !c.owner), "an instance moderator's group");
    assert!(!cards.iter().any(|c| c.name == "Viewers"), "view-all alone is not listed");
    let unnamed: Vec<String> = cards.iter().filter(|c| !c.named).map(|c| c.id.clone()).collect();
    assert_eq!(unnamed.len(), 8);
    let Reply::Hydrated(named) = ask(&c, Request::Hydrate { ids: unnamed }) else { panic!() };
    assert_eq!(named.iter().filter(|(_, c)| c.is_ok()).count(), 7, "the others are named");
    assert_eq!(named[2].1, Err(Failure::Api(GroupFailure::NotFound)));
    assert_eq!(named[0].1.as_ref().unwrap().member_count, Some(0));
    assert_eq!(named[3].1.as_ref().unwrap().member_count, None, "not given is not zero");

    let (c, _) = ctl(vec![ok(json!({})), ok(json!([]))]);
    assert_eq!(ask(&c, Request::Discover), Reply::Discovered(vec![]), "zero groups");
    let (c, _) = ctl(vec![Scripted::Answer(503, json!({}))]);
    assert_eq!(ask(&c, Request::Discover), Reply::Failed(Failure::Api(GroupFailure::Unavailable)), "a failure, not zero");
}

/// B3 and r21: a cached account while connecting grants nothing, and nothing is asked.
#[test]
fn connecting_grants_nothing() {
    let (c, sent) = ctl(vec![]);
    c.update(|s| {
        s.vrchat_login.known = s.vrchat_login.user.take();
        s.vrchat_login.restoring = true;
    });
    assert_eq!(ask(&c, Request::Discover), Reply::Failed(Failure::SignedOut));
    assert!(sent.lock().unwrap().is_empty());
}

/// Every write there is, carried out: exactly one request each, the ledger's entry done by VRChat's own answer.
#[test]
fn every_write_goes_once_and_is_recorded() {
    for kind in Kind::ALL {
        let (c, w, sent) = world();
        let s = prepare(&c, the(&c, kind)).unwrap_or_else(|r| panic!("{kind:?}: {r:?}"));
        assert_eq!(s.group_name, "Made-up Makers", "the group's name as read");
        assert!(s.confirm, "{kind:?} is confirmed");
        assert!(matches!(commit(&c, &s), Outcome::Done(_)), "{kind:?}");
        assert_eq!(writes(&sent).len(), 1, "{kind:?}");
        assert_eq!(w.lock().unwrap().done.len(), 1);
        let e = entry(&c, &s.op);
        assert_eq!((e.state, e.by, e.kind), (State::Done, Some(By::Answer), kind));
        assert!(!kind.makes() || e.made.is_some(), "{kind:?}: what it made is known");
        assert_eq!(commit(&c, &s), Outcome::Refused(Failure::Duplicate), "one operation goes once");
        assert_eq!(writes(&sent).len(), 1);
    }
}

/// verify2 B-B, per kind: only what identifies the very object settles anything; every look-alike leaves it unknown.
#[test]
fn a_check_concludes_only_from_the_very_object() {
    let mut rows = 0;
    for kind in Kind::ALL {
        // The right evidence, carried out or not.
        for done in [true, false] {
            let (c, w, sent) = world();
            let op = lost(&c, &w, kind, done);
            let soon = check(&c, &op);
            later(&c);
            let seen = if matches!(entry(&c, &op).state, State::Unknown) { check(&c, &op) } else { soon.clone() };
            let want = match (kind, done) {
                (Kind::DeletePost, true) => Observation::Inconclusive(Unclear::NotSeen),
                (_, true) => Observation::Done,
                (k, false) if k.makes() => Observation::Candidates(vec![]),
                (_, false) => Observation::NotDone,
            };
            assert_eq!(seen, want, "{kind:?}, done: {done}");
            if want == Observation::NotDone {
                assert_eq!(soon, Observation::Inconclusive(Unclear::TooSoon), "{kind:?}: read right after the send, absence says nothing");
            }
            let state = entry(&c, &op).state;
            assert_eq!(state, match want { Observation::Done => State::Done, Observation::NotDone => State::Refused, _ => State::Unknown }, "{kind:?}");
            assert_eq!(writes(&sent).len(), 1, "a check sends nothing");
        }
        // Every look-alike, the write in fact not carried out.
        let count = decoys(kind, None).len();
        for i in 0..count {
            let (c, w, _) = world();
            let op = lost(&c, &w, kind, false);
            later(&c);
            let md5 = entry(&c, &op).evidence.md5;
            let (name, decoy) = decoys(kind, md5).remove(i);
            decoy(&mut w.lock().unwrap());
            let seen = check(&c, &op);
            assert!(matches!(seen, Observation::Candidates(_) | Observation::Inconclusive(_)), "{kind:?}, {name}: {seen:?}");
            assert_eq!(entry(&c, &op).state, State::Unknown, "{kind:?}, {name}");
            rows += 1;
        }
    }
    assert!(rows >= 50, "{rows} look-alikes");
}

/// What the person says is kept as theirs: only after a check, only one of what they were shown, and a "not done" leaves
/// the entry unknown (the same may then go again, with a warning).
#[test]
fn the_persons_word_is_kept_as_theirs() {
    let (c, w, sent) = world();
    let op = lost(&c, &w, Kind::CreatePost, true);
    let say = |op: &str, said: Said, made: Option<&str>| ask(&c, Request::Said { op: op.into(), said, made: made.map(str::to_string) });
    assert_eq!(say(&op, Said::NotDone, None), Reply::Failed(Failure::NotKept), "not before a check");
    // The post is there with another audience (someone changed it): shown, not concluded.
    w.lock().unwrap().posts[0]["roleIds"] = json!([]);
    let Observation::Candidates(list) = check(&c, &op) else { panic!() };
    assert_eq!((list.len(), list[0].same), (1, false));
    assert_eq!(say(&op, Said::Done, Some("gpos_1")), Reply::Failed(Failure::Api(GroupFailure::Invalid)), "not one of those shown");
    assert_eq!(say(&op, Said::Done, None), Reply::Failed(Failure::Api(GroupFailure::Invalid)));
    assert_eq!(prepare(&c, the(&c, Kind::CreatePost)), Err(Reply::MaySent { op: op.clone() }), "the same does not go meanwhile");
    assert_eq!(say(&op, Said::NotDone, None), Reply::Said { op: op.clone() });
    let e = entry(&c, &op);
    assert_eq!((e.state, e.said), (State::Unknown, Some(Said::NotDone)), "a word, not a result");
    let again = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert_eq!((again.warning, again.confirm), (Some(Warning::SaidNotDone), true));
    assert_eq!(writes(&sent).len(), 1);

    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::CreatePost, true);
    w.lock().unwrap().posts[0]["roleIds"] = json!([]);
    let Observation::Candidates(list) = check(&c, &op) else { panic!() };
    assert_eq!(ask(&c, Request::Said { op: op.clone(), said: Said::Done, made: Some(list[0].id.clone()) }), Reply::Said { op: op.clone() });
    let e = entry(&c, &op);
    assert_eq!((e.state, e.by, e.made.as_deref()), (State::Done, Some(By::Person), Some(list[0].id.as_str())));
}

/// The same thing is refused while it may have gone; another to the same object is warned of and confirmed; a second of
/// the very same post within a day is warned of.
#[test]
fn what_may_have_gone_is_not_sent_again_unseen() {
    for kind in Kind::ALL {
        let (c, w, sent) = world();
        // An upload's own bytes again, staged anew.
        let write = the(&c, kind);
        let again = match &write {
            Write::Upload { tag, group, staged, art } => Write::Upload { tag: *tag, group: group.clone(), staged: crate::pages::groups::stage_upload(&staged.read().unwrap()).unwrap(), art: *art },
            other => other.clone(),
        };
        let s = prepare(&c, write).unwrap();
        w.lock().unwrap().fault = Some(Fault::Drop { done: false });
        assert_eq!(commit(&c, &s), Outcome::Unknown);
        assert_eq!(prepare(&c, again), Err(Reply::MaySent { op: s.op.clone() }), "{kind:?}");
        assert_eq!(writes(&sent).len(), 1, "{kind:?}");
    }
    let (c, w, _) = world();
    lost(&c, &w, Kind::EditPost, false);
    let Write::EditPost { group, baseline, edit } = the(&c, Kind::EditPost) else { panic!() };
    let other = prepare(&c, Write::EditPost { group: group.clone(), baseline: baseline.clone(), edit: PostDraft { title: "Another".into(), ..edit } }).unwrap();
    assert_eq!((other.warning, other.confirm), (Some(Warning::AnotherMayHaveGone), true));
    let gone = prepare(&c, Write::DeletePost { group, post: baseline }).unwrap();
    assert_eq!(gone.warning, Some(Warning::AnotherMayHaveGone), "its deletion is of the same post");
    let (c, _, _) = world();
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    let twice = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert_eq!((twice.warning, twice.confirm), (Some(Warning::AlreadyDone), true));
    // A cosmetic role, given with no confirmation, is confirmed too while another change of that person may have gone.
    let (c, w, _) = world();
    let plain = |role: &str| Write::MemberRole { group: G.into(), user: U.into(), role: role.into(), add: true };
    assert!(!prepare(&c, plain("grol_artist")).unwrap().confirm, "a role with no rights");
    let s = prepare(&c, plain("grol_artist")).unwrap();
    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
    assert_eq!(commit(&c, &s), Outcome::Unknown);
    let s = prepare(&c, plain("grol_events")).unwrap();
    assert_eq!((s.warning, s.confirm), (Some(Warning::AnotherMayHaveGone), true));
}

/// Nothing is sent whose record cannot be kept first.
#[test]
fn a_write_that_cannot_be_recorded_is_not_sent() {
    let (c, _, sent) = world();
    c.groups_book().full.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(prepare(&c, the(&c, Kind::CreatePost)), failed(Failure::NotKept));
    c.groups_book().full.store(false, std::sync::atomic::Ordering::Relaxed);
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    c.groups_book().full.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(commit(&c, &s), Outcome::Refused(Failure::NotKept));
    assert!(writes(&sent).is_empty());
}

/// verify2 B-E, B-F, design 5: what goes over the wire is the sealed payload, whatever everything else becomes; what was
/// shown otherwise than it is sealed does not go; a window has one sealed write at a time; another window's is not its.
#[test]
fn what_is_sealed_is_what_goes() {
    for kind in Kind::ALL {
        let (c, w, sent) = world();
        let s = prepare(&c, the(&c, kind)).unwrap();
        let sealed = entry(&c, &s.op).payload;
        // Other requests of the same window meanwhile, and the world's other contents.
        let _ = ask(&c, Request::Open { group: G.into() });
        let _ = ask(&c, Request::Discover);
        w.lock().unwrap().members.insert(TWIN.into(), super::world::member(TWIN, &[]));
        let shown = s.clone();
        assert!(matches!(commit(&c, &shown), Outcome::Done(_)), "{kind:?}");
        let sent = sent.lock().unwrap();
        let out: Vec<&Sent> = sent.iter().filter(|r| r.method != "GET").collect();
        assert_eq!(out.len(), 1);
        let path = out[0].url.trim_start_matches("https://api.vrchat.cloud/api/1").trim_start_matches("/groups/").to_string();
        let wire = match &out[0].upload {
            Some((tag, bytes)) => g::digest(&["POST", &format!("file/image/{tag}"), &sha256(bytes)]),
            None => g::digest(&[&out[0].method, &path, out[0].body.as_deref().unwrap_or("")]),
        };
        assert_eq!(wire, sealed, "{kind:?}: the bytes on the wire are the sealed payload");
    }
    // Anything changed in what is shown, and it is not what was sealed.
    type Change = fn(&mut Sealed);
    let changes: [(&str, Change); 6] = [
        ("the group's name", |s| s.group_name = "Another group".into()),
        ("the group", |s| s.group = H.into()),
        ("the picture", |s| s.art.as_mut().unwrap().sha256 = "0".repeat(64)),
        ("the file", |s| if let Write::ApplyArt { file, .. } = &mut s.write { *file = "file_other".into() }),
        ("the slot", |s| if let Write::ApplyArt { slot, .. } = &mut s.write { *slot = ArtSlot::Icon }),
        ("the warning", |s| s.warning = Some(Warning::AlreadyDone)),
    ];
    for (what, change) in changes {
        let (c, _, sent) = world();
        let mut s = prepare(&c, the(&c, Kind::ApplyBanner)).unwrap();
        change(&mut s);
        assert_eq!(commit(&c, &s), Outcome::Refused(Failure::Changed), "{what}");
        assert_eq!(entry(&c, &s.op).state, State::Refused);
        assert!(writes(&sent).is_empty(), "{what}");
    }
    // One at a time, and each window its own.
    let (c, _, sent) = world();
    let first = prepare(&c, the(&c, Kind::ApplyBanner)).unwrap();
    let second = prepare(&c, the(&c, Kind::ApplyIcon)).unwrap();
    assert_eq!(commit(&c, &first), Outcome::Refused(Failure::Expired), "replaced by the next");
    assert_eq!((entry(&c, &first.op).state, entry(&c, &first.op).by), (State::Refused, Some(By::NeverSent)));
    let other = c.groups(LINK + 1, scope(&c), Request::Commit { op: second.op.clone(), digest: second.digest() });
    assert_eq!(other, Reply::Outcome { op: second.op.clone(), outcome: Outcome::Refused(Failure::Expired) });
    assert_eq!(ask(&c, Request::Cancel { op: second.op.clone() }), Reply::Cancelled);
    assert_eq!(commit(&c, &second), Outcome::Refused(Failure::Expired));
    let third = prepare(&c, the(&c, Kind::ApplyIcon)).unwrap();
    c.groups_link_closed(LINK);
    assert_eq!(commit(&c, &third), Outcome::Refused(Failure::Expired), "its window's link closed");
    assert!(writes(&sent).is_empty());
}

/// verify2 B-E: the picture of an apply's confirmation is the bytes VRChat serves for the very file, by their hash; an
/// upload's, the staged bytes.
#[test]
fn a_sealed_picture_is_the_files_own() {
    let (c, _, _) = world();
    for (kind, id) in [(Kind::ApplyBanner, "file_gal1"), (Kind::ApplyIcon, "file_ico1")] {
        let s = prepare(&c, the(&c, kind)).unwrap();
        let art = s.art.clone().unwrap();
        let Reply::Picture(bytes) = ask(&c, Request::SealedArt { op: s.op.clone() }) else { panic!() };
        assert_eq!(bytes, super::world::picture(id));
        assert_eq!((art.file.as_deref(), art.sha256, art.size), (Some(id), sha256(&bytes), bytes.len() as u64));
        assert_eq!(c.groups(LINK + 1, scope(&c), Request::SealedArt { op: s.op }), Reply::Failed(Failure::Expired), "its window's alone");
    }
    let write = the(&c, Kind::UploadIcon);
    let Write::Upload { staged, .. } = &write else { panic!() };
    let bytes = staged.read().unwrap();
    let s = prepare(&c, write).unwrap();
    assert_eq!(ask(&c, Request::SealedArt { op: s.op.clone() }), Reply::Picture(bytes.clone()));
    assert_eq!(s.art, Some(SealedArt { file: None, sha256: sha256(&bytes), size: bytes.len() as u64 }));
}

/// J3: a staged picture lives as long as what it is for: a refusal, a cancel, another prepare, the link, an end.
#[test]
fn a_staged_picture_goes_with_its_write() {
    let (c, w, _) = world();
    let path = |s: &Write| match s {
        Write::Upload { staged, .. } => staging_dir().join(&staged.name),
        _ => unreachable!(),
    };
    let write = the(&c, Kind::UploadIcon);
    w.lock().unwrap().tags.clear();
    assert_eq!(prepare(&c, write.clone()), failed(Failure::Withheld(Why::NotEligible)));
    assert!(!path(&write).exists(), "refused");
    w.lock().unwrap().tags = vec!["system_supporter".into()];
    let ends: [(&str, fn(&Shared, &Sealed)); 4] = [
        ("cancelled", |c, s| drop(ask(c, Request::Cancel { op: s.op.clone() }))),
        ("replaced", |c, _| drop(prepare(c, the(c, Kind::ApplyIcon)))),
        ("its link closed", |c, _| c.groups_link_closed(LINK)),
        ("sent", |c, s| drop(commit(c, s))),
    ];
    for (what, end) in ends {
        let write = the(&c, Kind::UploadIcon);
        let s = prepare(&c, write.clone()).unwrap();
        assert!(path(&write).exists());
        end(&c, &s);
        assert!(!path(&write).exists(), "{what}");
    }
    let old = staging_dir().join("00000000-0000-4000-8000-00000000feed.png");
    std::fs::write(&old, b"x").unwrap();
    std::fs::File::options().write(true).open(&old).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(7200)).unwrap();
    let _ = prepare(&c, the(&c, Kind::ApplyIcon));
    assert!(!old.exists(), "left by an earlier program");
}

/// verify2 B-G, design 6: an edit says only what the person changed, on the post just read. Someone else's change between
/// the confirmation and the last read stops it, whatever they changed; the post keeps what they made it.
#[test]
fn an_edit_never_writes_over_what_someone_else_just_changed() {
    type Other = fn(&mut World);
    let none = g::PostParts::default();
    let others: [(&str, Other, Failure); 5] = [
        ("restricted", |w| w.posts[0]["roleIds"] = json!(["grol_mod"]), Failure::PostChanged(g::PostParts { roles: true, ..none })),
        ("opened", |w| w.posts[0]["visibility"] = json!("public"), Failure::PostChanged(g::PostParts { visibility: true, ..none })),
        ("its picture removed", |w| w.posts[0]["imageId"] = Value::Null, Failure::PostChanged(g::PostParts { image: true, ..none })),
        ("its words changed", |w| w.posts[0]["text"] = json!("changed"), Failure::PostChanged(g::PostParts { text: true, ..none })),
        ("its audience unreadable", |w| w.posts[0]["roleIds"] = json!("?"), Failure::NotRead(Missing::Post)),
    ];
    for (what, other, want) in others {
        for kind in [Kind::EditPost, Kind::DeletePost] {
            let (c, w, sent) = world();
            let write = match (kind, the(&c, Kind::EditPost)) {
                (Kind::DeletePost, Write::EditPost { group, baseline, .. }) => Write::DeletePost { group, post: baseline },
                (_, w) => w,
            };
            let s = prepare(&c, write).unwrap();
            // After the group is read for the last time, before the post is: the last read is the post's own.
            w.lock().unwrap().then = vec![(format!("/groups/{G}?"), Box::new(other))];
            assert_eq!(commit(&c, &s), Outcome::Refused(want), "{kind:?}, {what}");
            assert!(writes(&sent).is_empty(), "{kind:?}, {what}");
            assert_eq!((entry(&c, &s.op).state, entry(&c, &s.op).by), (State::Refused, Some(By::BeforeSending)));
        }
    }
    // The wire of a typo: the title; the audience and picture exactly as read; no notification.
    let (c, _, sent) = world();
    let s = prepare(&c, the(&c, Kind::EditPost)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    let body: Value = serde_json::from_str(sent.lock().unwrap().last().unwrap().body.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"title": "Meet-up!", "text": OTHERS_WORDS, "visibility": "group", "roleIds": ["grol_artist"], "imageId": "file_pic"}));
}

/// Design 6: what is left open is one step in memory. With the real pacing on, nothing is asked, written to disk or waited
/// for between VRChat's answer to the last read and the request.
#[test]
fn the_last_read_and_the_send_are_one_step() {
    let mut worst = Duration::ZERO;
    for kind in [Kind::EditPost, Kind::DeletePost, Kind::EditRole, Kind::AssignRole, Kind::ApplyBanner] {
        let c = Controller::new(Settings::default());
        let w: Shown = Arc::new(Mutex::new(World::default()));
        let sent = c.test_vrchat_routed_paced(route(w.clone()));
        let s = prepare(&c, the(&c, kind)).unwrap();
        // A group write went out just now: the next one's pacing is still running.
        let first = prepare(&c, Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true }).unwrap();
        assert!(matches!(commit(&c, &first), Outcome::Done(_)));
        let s = match prepare(&c, s.write) {
            Ok(s) => s,
            Err(r) => panic!("{kind:?}: {r:?}"),
        };
        let outcome = commit(&c, &s);
        assert!(matches!(outcome, Outcome::Done(_)), "{kind:?}: {outcome:?}");
        let sent = sent.lock().unwrap();
        let at = sent.iter().rposition(|r| r.method != "GET").unwrap();
        let (last, write) = (&sent[at - 1], &sent[at]);
        let what = match kind {
            Kind::EditPost | Kind::DeletePost => "/posts?",
            Kind::AssignRole => "/members/",
            _ => "?includeRoles",
        };
        assert!(last.method == "GET" && last.url.contains(what), "{kind:?}: the last read is of the very object: {}", last.url);
        worst = worst.max(write.at - last.answered.unwrap());
    }
    println!("between the last read's answer and the request: {worst:?} at worst");
    assert!(worst < Duration::from_millis(50), "{worst:?}: nothing waits between them");
}

/// verify2 B-D: what VRChat leaves out is unknown, for every input and at both times: nothing is sent, and nothing is
/// called "no confirmation" for want of it.
#[test]
fn what_is_missing_is_never_allowed() {
    let without = |key: &'static str| -> Box<dyn Fn(&mut World)> {
        Box::new(move |w: &mut World| {
            let c = Controller::new(Settings::default());
            drop(c);
            let mut g = serde_json::to_value(json!({"id": G, "name": "Made-up Makers", "bannerId": w.banner, "iconId": w.icon, "roles": w.roles,
                "myMember": {"id": "gmem_me", "groupId": G, "userId": "usr_test", "roleIds": ["grol_owner"], "mRoleIds": [], "permissions": ["*"], "membershipStatus": "member"}})).unwrap();
            match key.split_once('.') {
                Some(("me", k)) => {
                    g["myMember"].as_object_mut().unwrap().remove(k);
                }
                Some(("me=", v)) => g["myMember"]["userId"] = json!(v),
                _ => {
                    g.as_object_mut().unwrap().remove(key);
                }
            }
            w.instead = vec![(format!("/groups/{G}?"), g)];
        })
    };
    let cases: Vec<(&str, Kind, Box<dyn Fn(&mut World)>, Missing)> = vec![
        ("no membership", Kind::CreatePost, without("myMember"), Missing::Membership),
        ("no rights", Kind::CreatePost, without("me.permissions"), Missing::Membership),
        ("no roles of mine", Kind::DeleteRole, without("me.roleIds"), Missing::Membership),
        ("no managed roles of mine", Kind::EditRole, without("me.mRoleIds"), Missing::Membership),
        ("no status", Kind::ApplyBanner, without("me.membershipStatus"), Missing::Membership),
        ("another account's membership", Kind::ApplyIcon, without("me=.usr_other"), Missing::Identity),
        ("no roles", Kind::AssignRole, without("roles"), Missing::Roles),
        ("no banner field", Kind::ApplyBanner, without("bannerId"), Missing::Picture),
        ("no icon field", Kind::ApplyIcon, without("iconId"), Missing::Picture),
        ("the role's rights unknown", Kind::AssignRole, Box::new(|w| drop(w.roles[1].as_object_mut().unwrap().remove("permissions"))), Missing::RoleDefinition),
        ("a right too long to carry", Kind::AssignRole, Box::new(|w| w.roles[1]["permissions"] = json!(["r".repeat(3000)])), Missing::RoleDefinition),
        ("the role's flag unknown", Kind::DeleteRole, Box::new(|w| drop(w.roles[3].as_object_mut().unwrap().remove("isManagementRole"))), Missing::RoleDefinition),
        ("my own role's place unknown", Kind::DeleteRole, Box::new(|w| drop(w.roles[0].as_object_mut().unwrap().remove("order"))), Missing::RoleOrder),
        ("the role's two-factor flag unknown", Kind::EditRole, Box::new(|w| drop(w.roles[2].as_object_mut().unwrap().remove("requiresTwoFactor"))), Missing::RoleDefinition),
        ("a right of the catalogue unreadable", Kind::CreateRole, Box::new(|w| w.instead = vec![("/permissions".into(), json!([{"name": "group-instance-join"}]))]), Missing::Catalogue),
        ("the member without managed roles", Kind::AssignRole, Box::new(|w| drop(w.members.get_mut(U).unwrap().as_object_mut().unwrap().remove("mRoleIds"))), Missing::Member),
        ("the member without a group", Kind::UnassignRole, Box::new(|w| drop(w.members.get_mut(V).unwrap().as_object_mut().unwrap().remove("groupId"))), Missing::Member),
        ("another member answered", Kind::AssignRole, Box::new(|w| w.instead = vec![(format!("/members/{U}"), super::world::member(TWIN, &[]))]), Missing::Member),
        ("the tags not given", Kind::UploadIcon, Box::new(|w| w.instead = vec![("/auth/user".into(), json!({"id": "usr_test"}))]), Missing::Entitlement),
        ("another account's tags", Kind::UploadIcon, Box::new(|w| w.instead = vec![("/auth/user".into(), json!({"id": "usr_other", "tags": ["system_supporter"]}))]), Missing::Entitlement),
    ];
    for (what, kind, change, missing) in &cases {
        let (c, w, sent) = world();
        let write = the(&c, *kind);
        change(&mut w.lock().unwrap());
        assert_eq!(prepare(&c, write), failed(Failure::NotRead(*missing)), "at the prepare: {what}");
        let (c, w, _) = world();
        let s = prepare(&c, the(&c, *kind)).unwrap();
        change(&mut w.lock().unwrap());
        let outcome = commit(&c, &s);
        // A role that lost its definition is also not the role that was confirmed: either stops it.
        assert!(matches!(outcome, Outcome::Refused(Failure::NotRead(_) | Failure::Changed | Failure::PostChanged(_) | Failure::RoleChanged(_))), "at the send: {what}: {outcome:?}");
        assert!(writes(&sent).is_empty(), "{what}");
    }
}

/// N2: a picture not among the first pages of a longer list is "not read", not "not yours"; one not in a list seen to its
/// end is not yours.
#[test]
fn a_picture_not_found_is_not_yet_not_yours() {
    let (c, w, _) = world();
    let apply = |file: &str| Write::ApplyArt { group: G.into(), slot: ArtSlot::Banner, file: file.into() };
    assert_eq!(prepare(&c, apply("file_nobodys")), failed(Failure::Withheld(Why::NotYourPicture)));
    let many: Vec<(String, Value)> = (0..260).map(|i| ("gallery".to_string(), super::world::file(&format!("file_m{i}"), "gallery", "usr_test", b"x"))).collect();
    w.lock().unwrap().files = many;
    assert_eq!(prepare(&c, apply("file_m250")), failed(Failure::NotRead(Missing::Picture)));
    assert!(prepare(&c, apply("file_m150")).is_ok());
}

/// J6: the owner's role keeps its two-factor flag and every other one; only its words change. N3: a post made public
/// leaves its restriction.
#[test]
fn fixed_flags_and_the_public_switch() {
    let (c, _, _) = world();
    let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
    let owner = page.group.roles.unwrap().into_iter().find(|r| r.id == "grol_owner").unwrap();
    let edit = |f: fn(&mut RoleDraft)| {
        let mut d = RoleDraft::of(&owner);
        f(&mut d);
        prepare(&c, Write::EditRole { group: G.into(), baseline: owner.clone(), edit: d })
    };
    assert_eq!(edit(|d| d.requires_two_factor = true), failed(Failure::Withheld(Why::OwnerRole)));
    assert_eq!(edit(|d| d.added_on_join = true), failed(Failure::Withheld(Why::OwnerRole)));
    assert!(edit(|d| d.name = "Founder".into()).is_ok());
    let draft = PostDraft { title: "t".into(), text: "x".into(), visibility: g::Visibility::Public, role_ids: vec!["grol_artist".into()], ..Default::default() };
    let s = prepare(&c, Write::CreatePost { group: G.into(), draft }).unwrap();
    assert!(matches!(&s.write, Write::CreatePost { draft, .. } if draft.role_ids.is_empty()), "what is shown, and goes, has no restriction");
    let odd = PostDraft { title: "t".into(), text: "x".into(), role_ids: vec!["not a role".into()], ..Default::default() };
    assert_eq!(prepare(&c, Write::CreatePost { group: G.into(), draft: odd }), failed(Failure::Api(GroupFailure::Invalid)));
}

/// Blocker 7: a role or post that is not what was confirmed is not deleted; one not whole is not deleted from here.
#[test]
fn a_deletion_whose_subject_changed_is_refused() {
    let (c, w, sent) = world();
    let s = prepare(&c, the(&c, Kind::DeleteRole)).unwrap();
    assert_eq!(s.role.as_ref().map(|r| r.name.as_str()), Some("Event goers"));
    w.lock().unwrap().roles[3] = role("grol_events", "Senior staff", 4, &["group-instance-moderate"], true);
    assert!(matches!(commit(&c, &s), Outcome::Refused(Failure::RoleChanged(p)) if p.name && p.permissions && !p.description));
    let Write::DeletePost { group, post } = the(&c, Kind::DeletePost) else { panic!() };
    assert_eq!(prepare(&c, Write::DeletePost { group: group.clone(), post: Post { complete: false, ..post.clone() } }), failed(Failure::NotRead(Missing::Post)));
    w.lock().unwrap().posts.clear();
    assert_eq!(prepare(&c, Write::DeletePost { group, post }), failed(Failure::NotRead(Missing::Post)), "one not found is not deleted blind");
    assert!(writes(&sent).is_empty());
}

/// B3: A → Connecting → B while a write waits for its confirmation: it can no longer be sent by anyone.
#[test]
fn an_account_switch_drops_a_sealed_write() {
    let (c, _, sent) = world();
    let a = scope(&c);
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    c.update(|s| s.vrchat_login.revision += 1);
    let commit = |scope: Scope| c.groups(LINK, scope, Request::Commit { op: s.op.clone(), digest: s.digest() });
    assert_eq!(commit(a), Reply::Failed(Failure::AccountChanged));
    assert_eq!(commit(scope(&c)), Reply::Outcome { op: s.op.clone(), outcome: Outcome::Refused(Failure::Expired) });
    let e = c.groups_book().get(&Key::of(&scope(&c)).unwrap(), &s.op).unwrap();
    assert_eq!((e.state, e.by), (State::Refused, Some(By::NeverSent)), "the same account, signed in again, sees it was never sent");
    assert!(writes(&sent).is_empty());
}

/// B3: the account changes while the request is in flight: the old window is told "unknown" and nothing of it; the ledger
/// has what VRChat answered, for that account when it is back.
#[test]
fn an_account_switch_after_transmission_shows_nothing_and_keeps_the_truth() {
    let (c, w, _) = world();
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    let hold = Arc::new(Hold::default());
    w.lock().unwrap().fault = Some(Fault::Held(hold.clone()));
    let (c2, a, op, digest) = (c.clone(), scope(&c), s.op.clone(), s.digest());
    let sending = std::thread::spawn(move || c2.groups(LINK, a, Request::Commit { op, digest }));
    assert!(hold.until_reached());
    c.update(|s| s.vrchat_login.revision += 1);
    hold.let_go();
    assert_eq!(sending.join().unwrap(), Reply::Outcome { op: s.op.clone(), outcome: Outcome::Unknown });
    assert_eq!(check(&c, &s.op), Observation::Recorded { done: true }, "the durable scope is the same account's");
}

/// M2 through the service: a moderator without the rights prepares nothing, and one that loses them between is stopped.
#[test]
fn rights_are_checked_on_fresh_data() {
    for kind in Kind::ALL.into_iter().filter(|k| k.tag().is_none()) {
        let (c, w, sent) = world();
        let write = the(&c, kind);
        w.lock().unwrap().rights = vec!["group-instance-moderate".into()];
        assert_eq!(prepare(&c, write), failed(Failure::Withheld(Why::NoPermission)), "{kind:?}");
        let (c, w, _) = world();
        let s = prepare(&c, the(&c, kind)).unwrap();
        w.lock().unwrap().rights = vec!["group-instance-moderate".into()];
        assert_eq!(commit(&c, &s), Outcome::Refused(Failure::Withheld(Why::NoPermission)), "{kind:?}");
        assert!(writes(&sent).is_empty());
    }
}

/// Major 1: a staged file that is not what was announced (another size or hash) is never sent.
#[test]
fn a_staged_upload_must_be_the_one_announced() {
    let (c, sent) = ctl(vec![]);
    let mut staged = crate::pages::groups::stage_upload(b"\x89PNG\r\n\x1a\nx").unwrap();
    staged.sha256 = "0".repeat(64);
    let up = |staged: &Staged| Write::Upload { tag: FileTag::Icon, group: G.into(), staged: staged.clone(), art: None };
    assert_eq!(prepare(&c, up(&staged)), failed(Failure::Changed));
    staged.name = "../../secret.png".into();
    assert_eq!(prepare(&c, up(&staged)), failed(Failure::Api(GroupFailure::Invalid)));
    assert!(sent.lock().unwrap().is_empty());
}

/// M9: nothing of groups is in the shared state; a reply survives the link's encoding.
#[test]
fn group_data_stays_out_of_the_snapshot_and_survives_the_link() {
    let (c, _, _) = world();
    let before = serde_json::to_string(&*c.view()).unwrap();
    let reply = ask(&c, Request::Open { group: G.into() });
    let s = prepare(&c, the(&c, Kind::EditPost)).unwrap();
    assert_eq!(serde_json::to_string(&*c.view()).unwrap(), before);
    let text = serde_json::to_string(&crate::ipc::ToClient::Groups { id: "x".into(), reply: reply.clone() }).unwrap();
    assert!(matches!(serde_json::from_str(&text).unwrap(), crate::ipc::ToClient::Groups { reply: r, .. } if r == reply));
    let back: Sealed = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!((back.digest(), back == s), (s.digest(), true), "a sealed write crosses the link whole");
}

#[test]
fn requests_are_bounded() {
    let (c, sent) = ctl(vec![]);
    let many: Vec<String> = (0..HYDRATE_BATCH + 1).map(|i| format!("grp_{i}")).collect();
    assert_eq!(ask(&c, Request::Hydrate { ids: many }), Reply::Failed(Failure::Api(GroupFailure::Invalid)));
    assert_eq!(c.groups_admit().map(drop), Some(()));
    assert!(sent.lock().unwrap().is_empty());
}

/// Design 3: every file the feature writes, after everything it does on a group with other people's posts in it: none of
/// their words; the ledger holds no id of another person but the member a role was given to.
#[test]
fn the_ledger_holds_no_words_and_no_one_else() {
    let (c, w, _) = world();
    for kind in Kind::ALL {
        let op = lost(&c, &w, kind, false);
        later(&c);
        let _ = check(&c, &op);
        let text = std::fs::read_to_string(c.groups_book().dir().join(format!("{op}.json"))).unwrap();
        for words in [OTHERS_WORDS, "Meet-up", "Saturday", "Bring your art", "Greeters", "Say hello", "Painters", "Sam", "Made-up", "Artists"] {
            assert!(!text.contains(words), "{kind:?}: {words} in {text}");
        }
        let people = [super::world::OTHERS_ID, TWIN, U, V].into_iter().filter(|p| text.contains(p)).collect::<Vec<_>>();
        let expected: &[&str] = match kind {
            Kind::AssignRole => &[U],
            Kind::UnassignRole => &[V],
            _ => &[],
        };
        assert_eq!(people, expected, "{kind:?}");
    }
}

// --- the second verification's own assertions on a check (verify2-groups-scratch/probe), in the new shapes -------------

/// The write as it would stand in the ledger, its answer lost, as the verifier asked `groups_reconcile` of it.
fn unknown_entry(c: &Shared, write: &Write, evidence: Evidence) -> String {
    let book = c.groups_book();
    let op = uuid::Uuid::new_v4().to_string();
    book.prepare(Entry { op: op.clone(), key: Key::of(&scope(c)).unwrap(), group: write.group().into(), kind: write.kind(), target: write.ids(), payload: write.payload(), evidence, state: State::Prepared, by: None, said: None, made: None, checked: false, prepared_at: book.now(), dispatched_at: None, ended_at: None }).unwrap();
    book.dispatch(&op).unwrap();
    book.end(&op, State::Unknown, None, None);
    later(c);
    op
}

#[test]
fn verify2_photo_removal_requires_observing_the_removal() {
    let (c, w, _) = world();
    let Write::EditPost { group, baseline, edit } = the(&c, Kind::EditPost) else { panic!() };
    let edit = PostDraft { title: baseline.title.clone(), image_id: None, ..edit };
    let s = prepare(&c, Write::EditPost { group, baseline, edit }).unwrap();
    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
    assert_eq!(commit(&c, &s), Outcome::Unknown);
    later(&c);
    assert_ne!(check(&c, &s.op), Observation::Done, "the picture is still there");
    assert_eq!(entry(&c, &s.op).state, State::Refused, "seen as it was: not done");
}
#[test]
fn verify2_duplicate_role_name_cannot_identify_a_creation() {
    let (c, _, _) = world();
    let w = Write::CreateRole { group: G.into(), draft: RoleDraft { name: "Artists".into(), description: "new role".into(), ..Default::default() } };
    let op = unknown_entry(&c, &w, Evidence { content: Some("0".into()), before: Some(vec![]), ..Default::default() });
    assert!(matches!(check(&c, &op), Observation::Candidates(_)), "inconclusive: for the person");
    assert_eq!(entry(&c, &op).state, State::Unknown);
}
#[test]
fn verify2_absent_art_field_is_inconclusive() {
    let (c, w, _) = world();
    w.lock().unwrap().instead = vec![(format!("/groups/{G}?"), json!({"id": G, "name": "x"}))];
    let op = unknown_entry(&c, &Write::ApplyArt { group: G.into(), slot: ArtSlot::Icon, file: "file_new".into() }, Evidence { previous: Some(Before { file: None }), ..Default::default() });
    assert_eq!(check(&c, &op), Observation::Inconclusive(Unclear::NotRead));
}
#[test]
fn verify2_disputed_member_must_be_the_requested_user() {
    let (c, w, _) = world();
    w.lock().unwrap().instead = vec![(format!("/members/{U}"), super::world::member(TWIN, &["grol_artist"]))];
    let op = unknown_entry(&c, &Write::MemberRole { group: G.into(), user: U.into(), role: "grol_artist".into(), add: true }, Evidence::default());
    assert_eq!(check(&c, &op), Observation::Inconclusive(Unclear::NotRead));
}
