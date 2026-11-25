//! Groups' service against a made-up VRChat (no network, no real group): the attack's, the two verifications' and the
//! design's outcomes. The window's side, and the matrix of every write and every failure, are in `pages/groups`.
use super::world::{decoys, role, route, scope, world, write_of, writes, Fault, Hold, Shown, World, G, OTHERS_ID, OTHERS_WORDS, TWIN, U, V};
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
fn of(c: &Shared, user: &str) -> Membership {
    super::world::membership_of(user, &|r| ask(c, r))
}
fn own_membership(c: &Shared) -> Membership {
    match ask(c, Request::Open { group: G.into() }) {
        Reply::Opened(p) => p.group.my_member.unwrap().membership().unwrap(),
        other => panic!("{other:?}"),
    }
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
    c.groups_book().get(&c.groups_key(&scope(c)).unwrap(), op).unwrap()
}
fn check(c: &Shared, op: &str) -> Observation {
    match ask(c, Request::Check { op: op.into() }) {
        Reply::Checked { observation, .. } => observation,
        other => panic!("{other:?}"),
    }
}
fn later(c: &Shared) {
    c.groups_book().ahead_ms.store(SETTLES_AFTER_MS + 1000, std::sync::atomic::Ordering::Relaxed);
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

/// verify3 Blocker 2, per kind: a check says what VRChat shows now and nothing else. It shows what was asked: closed,
/// whoever did it. It does not: the person's to decide, and nothing is called "not done". It cannot tell: nothing changes.
#[test]
fn a_check_says_what_vrchat_shows_now() {
    let mut rows = 0;
    for kind in Kind::ALL {
        for done in [true, false] {
            let (c, w, sent) = world();
            let op = lost(&c, &w, kind, done);
            let soon = check(&c, &op);
            later(&c);
            let seen = if entry(&c, &op).open() { check(&c, &op) } else { soon.clone() };
            let shape = match &seen {
                Observation::Candidates(l) => format!("{} shown, {} exactly it", l.len(), l.iter().filter(|c| c.like == Likeness::Exactly).count()),
                o => format!("{o:?}"),
            };
            let want = match (kind, done) {
                (Kind::DeletePost, true) => "CannotTell(NotSeen)",
                (k, true) if k.makes() => "1 shown, 1 exactly it",
                (k, false) if k.makes() => "0 shown, 0 exactly it",
                (_, true) => "Shows",
                (_, false) => "ShowsNot",
            };
            assert_eq!(shape, want, "{kind:?}, done: {done}");
            if want == "ShowsNot" {
                assert_eq!(soon, Observation::CannotTell(Unclear::TooSoon), "{kind:?}: read right after the send, absence says nothing");
            }
            let e = entry(&c, &op);
            let (answer, closed) = match want {
                "Shows" => (Answer::Shows, Some(Closing::Shows)),
                "ShowsNot" => (Answer::ShowsNot, None),
                _ => (Answer::CannotTell, None),
            };
            assert_eq!((e.state, e.answer, e.looked, e.closed), (State::Unknown, Some(answer), false, closed), "{kind:?}: what became of it stays unknown");
            assert_eq!(writes(&sent).len(), 1, "a check sends nothing");
            if want == "Shows" {
                assert_eq!(check(&c, &op), Observation::Closed, "settled: not read again");
            }
        }
        // Every look-alike, the write in fact not carried out.
        let count = decoys(kind, (None, None)).len();
        for i in 0..count {
            let (c, w, _) = world();
            let op = lost(&c, &w, kind, false);
            later(&c);
            let e = entry(&c, &op);
            let (name, decoy) = decoys(kind, (e.evidence.md5, e.evidence.size)).remove(i);
            decoy(&mut w.lock().unwrap());
            let seen = check(&c, &op);
            assert!(matches!(seen, Observation::Candidates(_) | Observation::CannotTell(_) | Observation::ShowsNot), "{kind:?}, {name}: {seen:?}");
            let e = entry(&c, &op);
            assert!(e.open() && e.answer != Some(Answer::Shows), "{kind:?}, {name}");
            rows += 1;
        }
        // VRChat refuses the read: it cannot tell, and the person still has a way out (verify3 Major 3).
        let (c, w, _) = world();
        let op = lost(&c, &w, kind, true);
        later(&c);
        w.lock().unwrap().refuses = true;
        assert_eq!(check(&c, &op), Observation::CannotTell(Unclear::Failed(Failure::Api(GroupFailure::Forbidden))), "{kind:?}");
        assert!(entry(&c, &op).checked);
        assert_eq!(decide(&c, &op, Choice::Drop), Reply::Decided { op: op.clone() }, "{kind:?}");
        assert_eq!(entry(&c, &op).closed, Some(Closing::Dropped));
        assert_eq!(check(&c, &op), Observation::Closed);
    }
    assert!(rows >= 60, "{rows} look-alikes");
}
fn decide(c: &Shared, op: &str, choice: Choice) -> Reply {
    ask(c, Request::Decide { op: op.into(), choice })
}

/// What someone else did since is what VRChat shows, and no more: a role given by another moderator closes it as "shows"
/// with no one credited; one taken back since is "does not show", never "was not done"; and sending it again is a new
/// operation, warned of, confirmed.
#[test]
fn what_others_did_is_what_is_shown_and_no_one_is_credited() {
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::AssignRole, false);
    w.lock().unwrap().members.get_mut(U).unwrap()["roleIds"] = json!(["grol_mod"]);
    assert_eq!(check(&c, &op), Observation::Shows);
    let e = entry(&c, &op);
    assert_eq!((e.state, e.by, e.closed), (State::Unknown, None, Some(Closing::Shows)));
    let (c, w, sent) = world();
    let op = lost(&c, &w, Kind::EditRole, true);
    w.lock().unwrap().roles = World::default().roles;
    later(&c);
    assert_eq!(check(&c, &op), Observation::ShowsNot);
    assert_eq!(prepare(&c, the(&c, Kind::EditRole)), Err(Reply::MaySent { op: op.clone() }), "held until the person decides");
    assert_eq!(decide(&c, &op, Choice::Again), Reply::Decided { op: op.clone() });
    let s = prepare(&c, the(&c, Kind::EditRole)).unwrap();
    assert_eq!((s.warning, s.confirm, s.op != op), (Some(Warning::SameBefore), true, true), "a new operation");
    assert_eq!(writes(&sent).len(), 1, "nothing goes by the decision itself");
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
}

/// What the person decides is kept as theirs: only after a check was asked for, only one of what they were shown, "again"
/// only when a look says it is not there; the entry stays unknown, and the same again is warned of.
#[test]
fn the_persons_decision_is_kept_as_theirs() {
    let (c, w, sent) = world();
    let op = lost(&c, &w, Kind::CreatePost, true);
    for choice in [Choice::Shows { made: None }, Choice::ShowsNot, Choice::Again, Choice::Drop] {
        assert_eq!(decide(&c, &op, choice.clone()), Reply::Failed(Failure::LookNotKept), "not before a check: {choice:?}");
    }
    // The post is there with another audience (someone changed it): shown, not concluded.
    w.lock().unwrap().posts[0]["roleIds"] = json!([]);
    let Observation::Candidates(list) = check(&c, &op) else { panic!() };
    assert_eq!((list.len(), list[0].like), (1, Likeness::NotExactly));
    assert_eq!(decide(&c, &op, Choice::Shows { made: Some("gpos_1".into()) }), Reply::Failed(Failure::Api(GroupFailure::Invalid)), "not one of those shown");
    assert_eq!(decide(&c, &op, Choice::Again), Reply::Failed(Failure::LookNotKept), "a check that cannot tell is no ground for again");
    assert_eq!(prepare(&c, the(&c, Kind::CreatePost)), Err(Reply::MaySent { op: op.clone() }), "the same does not go meanwhile");
    assert_eq!(decide(&c, &op, Choice::ShowsNot), Reply::Decided { op: op.clone() });
    let e = entry(&c, &op);
    assert_eq!((e.state, e.answer, e.looked, e.open()), (State::Unknown, Some(Answer::ShowsNot), true, true), "their look, and still held");
    assert_eq!(decide(&c, &op, Choice::Again), Reply::Decided { op: op.clone() });
    let again = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert_eq!((again.warning, again.confirm), (Some(Warning::SameBefore), true));
    assert_eq!(writes(&sent).len(), 1);
    assert_eq!(decide(&c, &op, Choice::Drop), Reply::Failed(Failure::LookNotKept), "closed once");

    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::CreatePost, true);
    w.lock().unwrap().posts[0]["roleIds"] = json!([]);
    let Observation::Candidates(list) = check(&c, &op) else { panic!() };
    assert_eq!(decide(&c, &op, Choice::Shows { made: Some(list[0].id.clone()) }), Reply::Decided { op: op.clone() });
    let e = entry(&c, &op);
    assert_eq!((e.state, e.closed, e.looked, e.made.as_deref()), (State::Unknown, Some(Closing::Shows), true, Some(list[0].id.as_str())));
    assert_eq!(prepare(&c, the(&c, Kind::CreatePost)).unwrap().warning, Some(Warning::AlreadyDone));
    assert!(c.groups.lock().unwrap().candidates.is_empty(), "what was offered goes with the decision");
    // Seen on vrchat.com and not among those shown: theirs to say, with no id.
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::CreateRole, false);
    assert_eq!(check(&c, &op), Observation::Candidates(vec![]));
    assert_eq!(decide(&c, &op, Choice::Shows { made: None }), Reply::Decided { op: op.clone() });
    assert_eq!((entry(&c, &op).closed, entry(&c, &op).made), (Some(Closing::Shows), None));
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
    lost(&c, &w, Kind::EditRole, false);
    let Write::EditRole { group, baseline, edit, .. } = the(&c, Kind::EditRole) else { panic!() };
    let other = prepare(&c, Write::EditRole { group: group.clone(), baseline: baseline.clone(), edit: RoleEdit { name: "Another".into(), ..edit }, back: None }).unwrap();
    assert_eq!((other.warning, other.confirm), (Some(Warning::AnotherMayHaveGone), true));
    let gone = prepare(&c, Write::DeleteRole { group, role: baseline.id }).unwrap();
    assert_eq!(gone.warning, Some(Warning::AnotherMayHaveGone), "its deletion is of the same role");
    let (c, _, _) = world();
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    let twice = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert_eq!((twice.warning, twice.confirm), (Some(Warning::AlreadyDone), true));
    // A cosmetic role, given with no confirmation, is confirmed too while another change of that person may have gone.
    let (c, w, _) = world();
    let plain = |role: &str| Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: role.into(), add: true };
    assert!(!prepare(&c, plain("grol_artist")).unwrap().confirm, "a role with no rights");
    let s = prepare(&c, plain("grol_artist")).unwrap();
    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
    assert_eq!(commit(&c, &s), Outcome::Unknown);
    let s = prepare(&c, plain("grol_events")).unwrap();
    assert_eq!((s.warning, s.confirm), (Some(Warning::AnotherMayHaveGone), true));
}

/// A write sealed by another window (or any other surface of the service), and its end.
fn seal(c: &Shared, link: u64, write: Write) -> Sealed {
    match c.groups(link, scope(c), Request::Prepare { write }) {
        Reply::Prepared(s) => *s,
        r => panic!("{r:?}"),
    }
}
fn go(c: &Shared, link: u64, s: &Sealed) -> Outcome {
    match c.groups(link, scope(c), Request::Commit { op: s.op.clone(), digest: s.digest() }) {
        Reply::Outcome { outcome, .. } => outcome,
        r => panic!("{r:?}"),
    }
}

/// verify3 Blocker 3: whatever was sealed at the same time by other windows, one of the same goes; the others are told, and
/// must confirm a deliberately different operation.
#[test]
fn of_the_same_in_several_windows_one_goes() {
    for (what, fault) in [("done", None), ("unknown", Some(Fault::Drop { done: true })), ("unknown, not done", Some(Fault::Drop { done: false }))] {
        for kind in Kind::ALL {
            let (c, w, sent) = world();
            let write = the(&c, kind);
            let copy = |w: &Write| match w {
                Write::Upload { tag, group, staged, art } => Write::Upload { tag: *tag, group: group.clone(), staged: crate::pages::groups::stage_upload(&staged.read().unwrap()).unwrap(), art: *art },
                other => other.clone(),
            };
            let (w2, w3) = (copy(&write), copy(&write));
            let (one, two, three) = (seal(&c, 7, write), seal(&c, 8, w2), seal(&c, 9, w3));
            assert_eq!((one.warning, two.warning, three.warning), (None, None, None));
            w.lock().unwrap().fault = fault.clone();
            let first = go(&c, 7, &one);
            assert_eq!(matches!(first, Outcome::Done(_)), fault.is_none(), "{kind:?}: {first:?}");
            for (link, s) in [(8, &two), (9, &three)] {
                let second = go(&c, link, s);
                assert!(matches!(second, Outcome::Refused(Failure::Since | Failure::MaySent | Failure::Changed | Failure::RoleChanged(_) | Failure::Api(GroupFailure::NotFound))), "{kind:?}, the first {what}: {second:?}");
            }
            assert_eq!(writes(&sent).len(), 1, "{kind:?}, the first {what}");
        }
    }
    // The same post in other bytes is the same post: another order, spaces, an accent apart, a notification or none.
    let post = |title: &str, roles: &[&str], notify: bool| Write::CreatePost {
        group: G.into(),
        draft: PostDraft { title: title.into(), text: "Bring your art".into(), role_ids: roles.iter().map(|r| r.to_string()).collect(), send_notification: notify, ..Default::default() },
        of: uuid::Uuid::new_v4().to_string(),
    };
    for (what, other) in [
        ("the roles in another order", post("Caf\u{e9}", &["grol_events", "grol_artist"], true)),
        ("spaces", post("  Caf\u{e9} ", &["grol_artist", "grol_events"], true)),
        ("an accent apart", post("Cafe\u{301}", &["grol_artist", "grol_events"], true)),
        ("something invisible", post("Caf\u{e9}\u{200b}", &["grol_artist", "grol_events"], true)),
        ("no notification", post("Caf\u{e9}", &["grol_artist", "grol_events"], false)),
    ] {
        let (c, _, sent) = world();
        let (one, two) = (seal(&c, 7, post("Caf\u{e9}", &["grol_artist", "grol_events"], true)), seal(&c, 8, other.clone()));
        assert!(matches!(go(&c, 7, &one), Outcome::Done(_)));
        assert_eq!(go(&c, 8, &two), Outcome::Refused(Failure::Since), "{what}");
        assert_eq!(writes(&sent).len(), 1, "{what}");
        let again = seal(&c, 8, other);
        assert_eq!((again.warning, again.confirm), (Some(Warning::AlreadyDone), true), "{what}: told, and confirmed");
        assert!(matches!(go(&c, 8, &again), Outcome::Done(_)), "{what}: on purpose");
        assert_eq!(writes(&sent).len(), 2);
    }
    for (what, other) in [("other words", post("Caf\u{e9}s", &["grol_artist", "grol_events"], true)), ("another audience", post("Caf\u{e9}", &["grol_artist"], true))] {
        let (c, _, sent) = world();
        let (one, two) = (seal(&c, 7, post("Caf\u{e9}", &["grol_artist", "grol_events"], true)), seal(&c, 8, other));
        assert!(matches!(go(&c, 7, &one), Outcome::Done(_)));
        assert!(matches!(go(&c, 8, &two), Outcome::Done(_)), "{what}: another post");
        assert_eq!(writes(&sent).len(), 2);
    }
    // Sealed before another change of the same person went out and may have been done: it was not told of it.
    let (c, w, sent) = world();
    let plain = |role: &str| Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: role.into(), add: true };
    let (one, two) = (seal(&c, 7, plain("grol_artist")), seal(&c, 8, plain("grol_events")));
    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
    assert_eq!(go(&c, 7, &one), Outcome::Unknown);
    assert_eq!(go(&c, 8, &two), Outcome::Refused(Failure::Since));
    assert_eq!(writes(&sent).len(), 1);
    assert_eq!(seal(&c, 8, plain("grol_events")).warning, Some(Warning::AnotherMayHaveGone));
}

/// verify3 Blocker 1: a role is given to, or taken from, the membership that was shown: one who left and joined again
/// between is another, at the send and at the check; so is one's own.
#[test]
fn a_role_change_is_for_one_membership() {
    type Change = fn(&mut Value);
    let changes: [(&str, Change); 3] = [("another membership", |m| m["id"] = json!("gmem_rejoined")), ("the same id, joined later", |m| m["joinedAt"] = json!("2026-11-23T10:00:00Z")), ("no time of joining any more", |m| drop(m.as_object_mut().unwrap().remove("joinedAt")))];
    for (what, change) in changes {
        for (kind, user) in [(Kind::AssignRole, U), (Kind::UnassignRole, V)] {
            let (c, w, sent) = world();
            let s = prepare(&c, the(&c, kind)).unwrap();
            let sealed = s.member.as_ref().unwrap().membership().unwrap();
            assert_eq!((sealed.id.as_str(), sealed.joined_at.as_str()), (format!("gmem_{}", &user[user.len() - 12..]).as_str(), "2025-05-05T05:05:05Z"), "sealed, and shown");
            change(w.lock().unwrap().members.get_mut(user).unwrap());
            let said = what != "no time of joining any more";
            assert_eq!(commit(&c, &s), Outcome::Refused(if said { Failure::Rejoined } else { Failure::NotRead(Missing::Member) }), "{kind:?}, {what}");
            assert!(writes(&sent).is_empty());
            let (c, w, _) = world();
            let op = lost(&c, &w, kind, true);
            change(w.lock().unwrap().members.get_mut(user).unwrap());
            later(&c);
            assert_eq!(check(&c, &op), Observation::CannotTell(if said { Unclear::Rejoined } else { Unclear::NotRead }), "{kind:?}, {what}");
            assert!(entry(&c, &op).open());
        }
    }
    // One's own roles: the account's own membership.
    let (c, w, sent) = world();
    let own = |add: bool| Write::MemberRole { group: G.into(), user: "usr_test".into(), membership: own_membership(&c), role: "grol_artist".into(), add };
    let s = prepare(&c, own(true)).unwrap();
    let mut g = w.lock().unwrap().group();
    g["myMember"]["id"] = json!("gmem_me_again");
    w.lock().unwrap().instead = vec![(format!("/groups/{G}?"), g)];
    assert_eq!(commit(&c, &s), Outcome::Refused(Failure::Rejoined));
    assert!(writes(&sent).is_empty());
    let s = prepare(&c, own(true)).unwrap();
    assert_eq!(s.member.unwrap().membership_id, "gmem_me_again", "looked at again: the new one");
}

/// The record is the Orbiters member's, by their id: renamed, what they sent is still held and shown; another member with
/// the name they had sees and is held by nothing of it.
#[test]
fn the_record_follows_the_member_not_the_name() {
    let as_member = |c: &Shared, id: &str, name: &str| {
        {
            let mut rt = c.account_runtime.lock().unwrap();
            rt.read = Some((orbiters_core::account::Server::current(), None));
            let account = rt.account.as_mut().unwrap();
            (account.user, account.username) = (Some(id.into()), Some(name.into()));
        }
        // VRChat's session is theirs, verified for them, as the sign-in publishes it.
        c.test_signed_in();
        c.update(|s| s.account.state = crate::state::AccountState::SignedIn { username: name.into() });
        let stamp = c.vrchat_now().unwrap();
        assert!(c.publish_login(&stamp, |st, _| (st.allowed, st.user) = (true, Some(orbiters_core::vrchat_api::User { id: "usr_test".into(), ..Default::default() }))));
    };
    let (c, w, _) = world();
    as_member(&c, "1", "tester");
    let op = lost(&c, &w, Kind::CreatePost, false);
    let shown = |c: &Shared| match ask(c, Request::Ledger { also: vec![] }) {
        Reply::Ledger { entries, .. } => entries.iter().any(|e| e.op == op),
        r => panic!("{r:?}"),
    };
    // Renamed, before anything is verified again: still theirs.
    c.update(|s| s.account.state = crate::state::AccountState::SignedIn { username: "Renamed".into() });
    assert!(shown(&c));
    as_member(&c, "1", "Renamed");
    assert!(shown(&c));
    assert_eq!(prepare(&c, the(&c, Kind::CreatePost)), Err(Reply::MaySent { op: op.clone() }));
    as_member(&c, "2", "tester");
    assert!(!shown(&c));
    assert!(prepare(&c, the(&c, Kind::CreatePost)).is_ok());
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
    assert_eq!(s.art, Some(SealedArt { file: None, sha256: sha256(&bytes), size: bytes.len() as u64, version: None }));
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
    let _ = ask(&c, Request::Ledger { also: vec![] });
    assert!(!old.exists(), "left by an earlier program, gone as soon as a page shows");
    // Refused before it was looked at: another account, not signed in, too busy.
    for (what, change) in [("another sign-in", (|c| c.update(|s| s.vrchat_login.revision += 1)) as fn(&Shared)), ("too busy", |c| c.groups.lock().unwrap().running = RUNNING_MAX)] {
        let (c, _, _) = world();
        let write = the(&c, Kind::UploadIcon);
        let then = scope(&c);
        change(&c);
        assert!(matches!(c.groups(LINK, then, Request::Prepare { write: write.clone() }), Reply::Failed(_)), "{what}");
        assert!(!path(&write).exists(), "{what}");
    }
}

/// Blocker 7, verify3 Major 5: a post is deleted only as it was shown: whatever someone changed of it between the
/// confirmation and the last read stops it.
#[test]
fn a_deletion_never_takes_what_someone_just_changed() {
    type Other = fn(&mut World);
    let none = g::PostParts::default();
    let others: [(&str, Other, Failure); 7] = [
        ("restricted", |w| w.posts[0]["roleIds"] = json!(["grol_mod"]), Failure::PostChanged(g::PostParts { roles: true, ..none })),
        ("opened", |w| w.posts[0]["visibility"] = json!("public"), Failure::PostChanged(g::PostParts { visibility: true, ..none })),
        ("its picture removed", |w| w.posts[0]["imageId"] = Value::Null, Failure::PostChanged(g::PostParts { image: true, ..none })),
        ("its words changed", |w| w.posts[0]["text"] = json!("changed"), Failure::PostChanged(g::PostParts { text: true, ..none })),
        ("its audience unreadable", |w| w.posts[0]["roleIds"] = json!("?"), Failure::NotRead(Missing::Post)),
        ("its text gone from the answer", |w| drop(w.posts[0].as_object_mut().unwrap().remove("text")), Failure::NotRead(Missing::Post)),
        ("its title not a text", |w| w.posts[0]["title"] = json!(5), Failure::NotRead(Missing::Post)),
    ];
    for (what, other, want) in others {
        let (c, w, sent) = world();
        let Reply::Posts(page) = ask(&c, Request::Posts { group: G.into(), offset: 0 }) else { panic!() };
        let s = prepare(&c, Write::DeletePost { group: G.into(), post: page.items[0].clone() }).unwrap();
        // After the group is read for the last time, before the post is: the last read is the post's own.
        w.lock().unwrap().then = vec![(format!("/groups/{G}?"), Box::new(other))];
        assert_eq!(commit(&c, &s), Outcome::Refused(want), "{what}");
        assert!(writes(&sent).is_empty(), "{what}");
        assert_eq!((entry(&c, &s.op).state, entry(&c, &s.op).by), (State::Refused, Some(By::BeforeSending)));
        // Sealed on a post already so: not offered.
        if matches!(want, Failure::NotRead(_)) {
            let (c, w, _) = world();
            other(&mut w.lock().unwrap());
            let Reply::Posts(page) = ask(&c, Request::Posts { group: G.into(), offset: 0 }) else { panic!() };
            assert!(!page.items[0].complete, "{what}");
            assert_eq!(prepare(&c, Write::DeletePost { group: G.into(), post: page.items[0].clone() }), failed(want));
        }
    }
}

/// verify3 Blocker 4: no write of this program edits a post: there is none to ask for, and what goes to a group's posts is a
/// creation or a deletion, whatever is asked of everything else.
#[test]
fn no_post_is_ever_edited_from_here() {
    for text in [r#"{"EditPost":{"group":"grp_1","baseline":{},"edit":{}}}"#, r#"{"editPost":{}}"#] {
        assert!(serde_json::from_str::<Write>(text).is_err());
    }
    assert!(serde_json::from_str::<Kind>(r#""edit_post""#).is_err());
    for kind in Kind::ALL {
        let (c, _, sent) = world();
        let s = prepare(&c, the(&c, kind)).unwrap();
        let _ = commit(&c, &s);
        let sent = sent.lock().unwrap();
        assert!(sent.iter().all(|r| !(r.method == "PUT" && r.url.contains("/posts"))), "{kind:?}");
        assert!(sent.iter().filter(|r| r.method != "GET").all(|r| !r.body.as_deref().unwrap_or("").contains("null")), "{kind:?}: and no null");
    }
}

/// Design 6: what is left open is one step in memory. With the real pacing on, nothing is asked, written to disk or waited
/// for between VRChat's answer to the last read and the request.
#[test]
fn the_last_read_and_the_send_are_one_step() {
    let mut worst = Duration::ZERO;
    for kind in [Kind::DeletePost, Kind::EditRole, Kind::AssignRole, Kind::ApplyBanner] {
        let c = Controller::new(Settings::default());
        let w: Shown = Arc::new(Mutex::new(World::default()));
        let sent = c.test_vrchat_routed_paced(route(w.clone()));
        let s = prepare(&c, the(&c, kind)).unwrap();
        // A group write went out just now: the next one's pacing is still running.
        let first = prepare(&c, Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: "grol_artist".into(), add: true }).unwrap();
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
            Kind::DeletePost => "/posts?",
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
            let mut g = w.group();
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
    let no = |key: &'static str, of: &'static str| -> Box<dyn Fn(&mut World)> { Box::new(move |w: &mut World| drop(w.members.get_mut(of).unwrap().as_object_mut().unwrap().remove(key))) };
    let no_file = |key: &'static str| -> Box<dyn Fn(&mut World)> {
        Box::new(move |w: &mut World| match key {
            "version" => drop(w.files[0].1["versions"][1].as_object_mut().unwrap().remove("version")),
            key => drop(w.files[0].1.as_object_mut().unwrap().remove(key)),
        })
    };
    let cases: Vec<(&str, Kind, Box<dyn Fn(&mut World)>, Missing)> = vec![
        ("no membership", Kind::CreatePost, without("myMember"), Missing::Membership),
        ("no rights", Kind::CreatePost, without("me.permissions"), Missing::Membership),
        ("no roles of mine", Kind::DeleteRole, without("me.roleIds"), Missing::Membership),
        ("no rights for a picture", Kind::ApplyBanner, without("me.permissions"), Missing::Membership),
        ("another account's membership", Kind::ApplyIcon, without("me=.usr_other"), Missing::Identity),
        ("no roles", Kind::AssignRole, without("roles"), Missing::Roles),
        ("no banner field", Kind::ApplyBanner, without("bannerId"), Missing::Picture),
        ("no icon field", Kind::ApplyIcon, without("iconId"), Missing::Picture),
        ("the role's rights unknown", Kind::DeleteRole, Box::new(|w| drop(w.roles[3].as_object_mut().unwrap().remove("permissions"))), Missing::RoleDefinition),
        ("a right too long to carry", Kind::EditRole, Box::new(|w| w.roles[2]["permissions"] = json!(["r".repeat(3000)])), Missing::RoleDefinition),
        ("whether it is Everyone's unreadable", Kind::DeleteRole, Box::new(|w| w.roles[3]["defaultRole"] = json!("no")), Missing::RoleDefinition),
        ("my own role's place unknown", Kind::DeleteRole, Box::new(|w| drop(w.roles[0].as_object_mut().unwrap().remove("order"))), Missing::RoleOrder),
        ("the role's two-factor flag unreadable", Kind::EditRole, Box::new(|w| w.roles[2]["requiresTwoFactor"] = json!(0)), Missing::RoleDefinition),
        ("a right of the catalogue unreadable", Kind::CreateRole, Box::new(|w| w.instead = vec![("/permissions".into(), json!([{"name": "group-instance-join"}]))]), Missing::Catalogue),
        ("the member without managed roles", Kind::AssignRole, no("mRoleIds", U), Missing::Member),
        ("the member without roles", Kind::UnassignRole, no("roleIds", V), Missing::Member),
        ("the member without a group", Kind::UnassignRole, no("groupId", V), Missing::Member),
        ("the member without a membership", Kind::AssignRole, no("id", U), Missing::Member),
        ("the picture without an owner", Kind::ApplyBanner, no_file("ownerId"), Missing::Picture),
        ("the picture without its kind", Kind::ApplyBanner, no_file("tags"), Missing::Picture),
        ("the picture without a version", Kind::ApplyBanner, no_file("version"), Missing::Picture),
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

/// verify3 deviation 9, the orchestrator's point 6: what VRChat leaves out withholds what needs it, and only that. Where
/// the strict side of a missing value is safe, the action still works, with it: confirmed, or with less rank.
#[test]
fn what_is_left_out_withholds_only_what_needs_it() {
    let mine = |gone: &'static [&'static str]| -> Box<dyn Fn(&mut World)> {
        Box::new(move |w: &mut World| {
            let mut g = w.group();
            gone.iter().for_each(|k| drop(g["myMember"].as_object_mut().unwrap().remove(*k)));
            w.instead = vec![(format!("/groups/{G}?"), g)];
        })
    };
    let role = |i: usize, key: &'static str| -> Box<dyn Fn(&mut World)> { Box::new(move |w: &mut World| drop(w.roles[i].as_object_mut().unwrap().remove(key))) };
    let cases: Vec<(&str, Kind, Box<dyn Fn(&mut World)>)> = vec![
        ("my managed roles and status left out: a post", Kind::CreatePost, mine(&["mRoleIds", "membershipStatus"])),
        ("all my roles left out: a post", Kind::DeletePost, mine(&["mRoleIds", "roleIds", "membershipStatus", "id", "joinedAt"])),
        ("all my roles left out: a picture", Kind::ApplyBanner, mine(&["mRoleIds", "roleIds", "membershipStatus"])),
        ("my managed roles left out: a role change, by those given by hand", Kind::EditRole, mine(&["mRoleIds", "membershipStatus"])),
        ("my managed roles left out: a role given", Kind::AssignRole, mine(&["mRoleIds"])),
        ("whether a role manages left out: given as a managing one", Kind::AssignRole, role(1, "isManagementRole")),
        ("whether members may take it left out: given", Kind::AssignRole, role(1, "isSelfAssignable")),
        ("its rights left out: given by the group's owner", Kind::AssignRole, role(1, "permissions")),
        ("its rank left out: given by the group's owner", Kind::AssignRole, role(1, "order")),
        ("a flag from after it was made left out: given", Kind::AssignRole, role(1, "requiresPurchase")),
        ("whether it is Everyone's left out: given", Kind::AssignRole, role(1, "defaultRole")),
    ];
    for (what, kind, change) in cases {
        let (c, w, sent) = world();
        let write = the(&c, kind);
        change(&mut w.lock().unwrap());
        let s = prepare(&c, write).unwrap_or_else(|r| panic!("{what}: {r:?}"));
        assert!(s.confirm, "{what}: confirmed");
        assert!(matches!(commit(&c, &s), Outcome::Done(_)), "{what}");
        assert_eq!(writes(&sent).len(), 1);
    }
    // Giving a cosmetic role whose management flag was left out is no longer "no confirmation".
    let (c, w, _) = world();
    role(2, "isManagementRole")(&mut w.lock().unwrap());
    assert!(prepare(&c, Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: "grol_artist".into(), add: true }).unwrap().confirm);
    // One's own roles need one's own record whole.
    for gone in [&["mRoleIds"][..], &["roleIds"], &["id"], &["joinedAt"]] {
        let (c, w, _) = world();
        let membership = own_membership(&c);
        mine(gone)(&mut w.lock().unwrap());
        let r = prepare(&c, Write::MemberRole { group: G.into(), user: "usr_test".into(), membership, role: "grol_artist".into(), add: true });
        assert!(matches!(r, Err(Reply::Failed(Failure::NotRead(Missing::Member | Missing::Membership)))), "{gone:?}: {r:?}");
    }
}

/// N2, verify3 Major 4: a picture is the account's own, of the slot's kind and the version shown, as VRChat says of that
/// very file; whichever way VRChat gives it, by its id or on the list of its kind.
#[test]
fn a_picture_applied_is_the_one_that_was_shown() {
    for by_id in [true, false] {
        let (c, w, _) = world();
        w.lock().unwrap().by_id = by_id;
        let apply = |file: &str| Write::ApplyArt { group: G.into(), slot: ArtSlot::Banner, file: file.into(), back: None };
        assert_eq!(prepare(&c, apply("file_nobodys")), failed(Failure::Withheld(Why::NotYourPicture)), "by id: {by_id}");
        let many: Vec<(String, Value)> = (0..260).map(|i| ("gallery".to_string(), super::world::file(&format!("file_m{i}"), "gallery", "usr_test", b"x"))).collect();
        w.lock().unwrap().files = many;
        assert!(prepare(&c, apply("file_m150")).is_ok());
        // Past the pages read, it is found by its id, or not read; never "not yours".
        let far = prepare(&c, apply("file_m250"));
        assert!(if by_id { far.is_ok() } else { far == failed(Failure::NotRead(Missing::Picture)) }, "by id: {by_id}: {far:?}");
        type Change = fn(&mut Value);
        let changes: [(&str, Change, Failure); 5] = [
            ("another's", |f| f["ownerId"] = json!(OTHERS_ID), Failure::Withheld(Why::NotYourPicture)),
            ("of another kind", |f| f["tags"] = json!(["icon"]), Failure::Withheld(Why::NotYourPicture)),
            ("another version", |f| {
                f["versions"][1]["version"] = json!(2);
                f["versions"][1]["file"]["url"] = json!("https://api.vrchat.cloud/api/1/file/file_gal1/2/file");
            }, Failure::Changed),
            ("other bytes, the same version", |f| f["versions"][1]["file"]["md5"] = json!("AAAAAAAAAAAAAAAAAAAAAA=="), Failure::Changed),
            ("its file gone", |f| f["versions"] = json!([{"version": 0}]), Failure::Changed),
        ];
        for (what, change, want) in changes {
            let (c, w, sent) = world();
            w.lock().unwrap().by_id = by_id;
            let s = prepare(&c, the(&c, Kind::ApplyBanner)).unwrap();
            assert_eq!(s.art.as_ref().unwrap().version, Some(1));
            change(&mut w.lock().unwrap().files[0].1);
            let outcome = commit(&c, &s);
            let unread = what == "its file gone" && outcome == Outcome::Refused(Failure::Api(GroupFailure::BadAnswer));
            assert!(outcome == Outcome::Refused(want) || unread, "by id: {by_id}, {what}: {outcome:?}");
            assert!(writes(&sent).is_empty(), "{what}");
            if matches!(want, Failure::Withheld(_)) {
                let (c, w, _) = world();
                w.lock().unwrap().by_id = by_id;
                change(&mut w.lock().unwrap().files[0].1);
                assert_eq!(prepare(&c, the(&c, Kind::ApplyBanner)), failed(want), "{what}");
            }
        }
    }
}

/// An upload is done by VRChat's own new file of the kind asked, whatever bytes it keeps of it: not by one that was there,
/// another's, of another kind, or, with no time said, against a list not read to its end.
#[test]
fn an_upload_is_its_new_file_not_its_bytes() {
    let answered = |of: &dyn Fn(&[u8]) -> Value, many: bool| {
        let (c, w, _) = world();
        if many {
            w.lock().unwrap().files = (0..260).map(|i| ("gallery".to_string(), super::world::file(&format!("file_m{i}"), "gallery", "usr_test", b"x"))).collect();
        }
        let write = the(&c, Kind::UploadGallery);
        let Write::Upload { staged, .. } = &write else { panic!() };
        let bytes = staged.read().unwrap();
        let s = prepare(&c, write).unwrap();
        w.lock().unwrap().fault = Some(Fault::Answer { done: false, status: 200, body: of(&bytes) });
        commit(&c, &s)
    };
    let file = super::world::file;
    let without_time = |mut f: Value| {
        f["versions"][1].as_object_mut().unwrap().remove("created_at");
        f
    };
    let old = |mut f: Value| {
        f["versions"][1]["created_at"] = json!("2020-01-01T00:00:00Z");
        f
    };
    for (what, outcome) in [
        ("re-encoded", answered(&|_| file("file_up9", "gallery", "usr_test", b"the same picture, in other bytes"), false)),
        ("the very bytes", answered(&|b| file("file_up9", "gallery", "usr_test", b), false)),
        ("no time, all there was read", answered(&|b| without_time(file("file_up9", "gallery", "usr_test", b)), false)),
        ("another clock, all there was read", answered(&|b| old(file("file_up9", "gallery", "usr_test", b)), false)),
    ] {
        assert!(matches!(&outcome, Outcome::Done(Done::Uploaded(f)) if f.id == "file_up9"), "{what}: {outcome:?}");
    }
    for (what, outcome) in [
        ("there before", answered(&|b| file("file_gal1", "gallery", "usr_test", b), false)),
        ("another's", answered(&|b| file("file_up9", "gallery", OTHERS_ID, b), false)),
        ("of another kind", answered(&|b| file("file_up9", "icon", "usr_test", b), false)),
        // More there than is read: no id is known to be new, whatever time it says (verify4 F2).
        ("made since, more there than is read", answered(&|_| file("file_up9", "gallery", "usr_test", b"other bytes"), true)),
        ("one of those past what is read, its version new", answered(&|_| file("file_m250", "gallery", "usr_test", b"x"), true)),
        ("no time, more there than is read", answered(&|b| without_time(file("file_up9", "gallery", "usr_test", b)), true)),
        ("old, more there than is read", answered(&|b| old(file("file_up9", "gallery", "usr_test", b)), true)),
    ] {
        assert_eq!(outcome, Outcome::Unknown, "{what}");
    }
}

/// verify4 F2: an entry whose picture cannot be shown was there all the same: an answer naming it is no receipt, and it is no
/// candidate; one that gives no id makes the list no list of all there was. A check says of each file how sure it is.
#[test]
fn every_entry_of_the_inventory_counts() {
    let bare = |id: Value| ("gallery".to_string(), json!({"id": id, "ownerId": "usr_test", "tags": ["gallery"], "versions": []}));
    let (c, w, _) = world();
    w.lock().unwrap().files.push(bare(json!("file_bare")));
    let s = prepare(&c, the(&c, Kind::UploadGallery)).unwrap();
    w.lock().unwrap().fault = Some(Fault::Answer { done: false, status: 200, body: super::world::file("file_bare", "gallery", "usr_test", b"old") });
    assert_eq!(commit(&c, &s), Outcome::Unknown);
    w.lock().unwrap().files[1] = ("gallery".into(), super::world::file("file_bare", "gallery", "usr_test", b"old"));
    assert_eq!(check(&c, &s.op), Observation::Candidates(vec![]), "its address is there now: it was there before");
    // How sure: of the kind and new by a whole list; untagged, undated, or with no whole list before: not.
    for (whole, want) in [(true, [Likeness::Since, Likeness::Uncertain]), (false, [Likeness::Uncertain, Likeness::Uncertain])] {
        let (c, w, _) = world();
        if !whole {
            w.lock().unwrap().files.push(bare(Value::Null));
        }
        let op = lost(&c, &w, Kind::UploadGallery, false);
        let mut untagged = super::world::file("file_untagged", "gallery", "usr_test", b"y");
        untagged.as_object_mut().unwrap().remove("tags");
        untagged["versions"][1].as_object_mut().unwrap().remove("created_at");
        w.lock().unwrap().files.extend([("gallery".into(), super::world::file("file_new", "gallery", "usr_test", b"x")), ("gallery".into(), untagged)]);
        let Observation::Candidates(list) = check(&c, &op) else { panic!() };
        assert_eq!(list.iter().map(|c| c.like).collect::<Vec<_>>(), want, "{whole}");
    }
    // A clock that is wrong hides nothing that a whole list says is new.
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::UploadGallery, false);
    let mut dated = super::world::file("file_dated", "gallery", "usr_test", b"x");
    dated["versions"][1]["created_at"] = json!("2020-01-01T00:00:00Z");
    w.lock().unwrap().files.push(("gallery".into(), dated));
    let Observation::Candidates(list) = check(&c, &op) else { panic!() };
    assert_eq!(list.iter().map(|c| (c.id.as_str(), c.like)).collect::<Vec<_>>(), [("file_dated", Likeness::Since)]);
}

/// An upload of which the answer was lost, kept by VRChat in bytes of its own: Check offers it, as added since.
#[test]
fn a_lost_upload_is_found_re_encoded() {
    for tag in [Kind::UploadGallery, Kind::UploadIcon] {
        let (c, w, _) = world();
        w.lock().unwrap().reencodes = true;
        let op = lost(&c, &w, tag, true);
        let Observation::Candidates(list) = check(&c, &op) else { panic!() };
        assert_eq!(list.iter().map(|c| (c.id.as_str(), c.like)).collect::<Vec<_>>(), [("file_up1", Likeness::Since)], "{tag:?}");
        assert!(entry(&c, &op).open(), "only the person says which it is");
        assert_eq!(decide(&c, &op, Choice::Shows { made: Some("file_up1".into()) }), Reply::Decided { op: op.clone() });
        assert!(!entry(&c, &op).open());
    }
}

/// The rights of a role that is there are sent by nothing: an edit's body has what the person changed, and no list; and
/// everyone's role, which has its rights alone to change, is not edited.
#[test]
fn rights_of_an_existing_role_are_never_sent() {
    let (c, _, sent) = world();
    let s = prepare(&c, the(&c, Kind::EditRole)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(Done::Roles(_))));
    let bodies: Vec<Value> = sent.lock().unwrap().iter().filter(|r| r.method == "PUT").map(|r| serde_json::from_str(r.body.as_deref().unwrap()).unwrap()).collect();
    assert_eq!(bodies, [json!({"name": "Painters", "requiresTwoFactor": true})]);
    let (c, w, _) = world();
    let id = {
        let mut w = w.lock().unwrap();
        w.roles[2]["defaultRole"] = json!(true);
        w.roles[2]["id"].as_str().unwrap().to_string()
    };
    let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
    let baseline = page.group.roles.unwrap().into_iter().find(|r| r.id == id).unwrap();
    let edit = RoleEdit { name: "Painters".into(), ..RoleEdit::of(&baseline) };
    assert_eq!(prepare(&c, Write::EditRole { group: G.into(), baseline, edit, back: None }), failed(Failure::Withheld(Why::FixedRole)));
}

/// Words the person left as they are, whatever VRChat let them be, are not sent back tidied or cut with a flag's change.
#[test]
fn words_left_as_they_are_are_not_sent_back() {
    let (c, w, sent) = world();
    w.lock().unwrap().roles[2]["name"] = json!(format!("  Artists\u{200b} {} ", "x".repeat(g::ROLE_NAME_MAX)));
    w.lock().unwrap().roles[2]["description"] = json!("long ".repeat(g::ROLE_DESCRIPTION_MAX));
    let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
    let baseline = page.group.roles.unwrap().into_iter().find(|r| r.id == "grol_artist").unwrap();
    let edit = RoleEdit { requires_two_factor: true, ..RoleEdit::of(&baseline) };
    let s = prepare(&c, Write::EditRole { group: G.into(), baseline, edit, back: None }).unwrap();
    commit(&c, &s);
    let bodies: Vec<Value> = sent.lock().unwrap().iter().filter(|r| r.method == "PUT").map(|r| serde_json::from_str(r.body.as_deref().unwrap()).unwrap()).collect();
    assert_eq!(bodies, [json!({"requiresTwoFactor": true})]);
}

/// Done, and VRChat's answer has another part of the role changed: it is said.
#[test]
fn a_role_whose_rights_moved_under_a_rename_is_said() {
    let (c, w, _) = world();
    let s = prepare(&c, the(&c, Kind::EditRole)).unwrap();
    let mut roles = w.lock().unwrap().roles.clone();
    let at = roles.iter().position(|r| r["id"] == "grol_artist").unwrap();
    roles[at]["name"] = json!("Painters");
    roles[at]["requiresTwoFactor"] = json!(true);
    roles[at]["permissions"] = json!(["group-bans-manage"]);
    w.lock().unwrap().fault = Some(Fault::Answer { done: true, status: 200, body: json!(roles) });
    assert!(matches!(commit(&c, &s), Outcome::Done(Done::Roles(_))));
    assert_eq!(entry(&c, &s.op).beside, Beside { moved: vec![Part::Rights], unread: vec![] });
}

/// The same of a group's picture.
#[test]
fn another_picture_moved_under_an_apply_is_said() {
    for (kind, beside) in [(Kind::ApplyBanner, true), (Kind::ApplyBanner, false)] {
        let (c, w, sent) = world();
        w.lock().unwrap().icon = json!("file_ico1");
        let s = prepare(&c, the(&c, kind)).unwrap();
        let mut body = w.lock().unwrap().group();
        body["bannerId"] = json!("file_gal1");
        if beside {
            body["iconId"] = Value::Null;
        }
        w.lock().unwrap().fault = Some(Fault::Answer { done: true, status: 200, body });
        assert!(matches!(commit(&c, &s), Outcome::Done(Done::Group(_))));
        assert_eq!(entry(&c, &s.op).beside, Beside { moved: if beside { vec![Part::Icon] } else { vec![] }, unread: vec![] });
        let bodies: Vec<Value> = sent.lock().unwrap().iter().filter(|r| r.method == "PUT").map(|r| serde_json::from_str(r.body.as_deref().unwrap()).unwrap()).collect();
        assert_eq!(bodies, [json!({"bannerId": "file_gal1"})], "only what the person chose");
    }
}

/// J6: the owner's role keeps its two-factor flag and every other one; only its words change. N3: a post made public
/// leaves its restriction.
#[test]
fn fixed_flags_and_the_public_switch() {
    let (c, _, _) = world();
    let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
    let owner = page.group.roles.unwrap().into_iter().find(|r| r.id == "grol_owner").unwrap();
    let edit = |f: fn(&mut RoleEdit)| {
        let mut d = RoleEdit::of(&owner);
        f(&mut d);
        prepare(&c, Write::EditRole { group: G.into(), baseline: owner.clone(), edit: d, back: None })
    };
    assert_eq!(edit(|d| d.requires_two_factor = true), failed(Failure::Withheld(Why::OwnerRole)));
    assert_eq!(edit(|d| d.added_on_join = true), failed(Failure::Withheld(Why::OwnerRole)));
    assert!(edit(|d| d.name = "Founder".into()).is_ok());
    let draft = PostDraft { title: "t".into(), text: "x".into(), visibility: g::Visibility::Public, role_ids: vec!["grol_artist".into()], ..Default::default() };
    let s = prepare(&c, Write::CreatePost { group: G.into(), draft, of: String::new() }).unwrap();
    assert!(matches!(&s.write, Write::CreatePost { draft, .. } if draft.role_ids.is_empty()), "what is shown, and goes, has no restriction");
    let odd = PostDraft { title: "t".into(), text: "x".into(), role_ids: vec!["not a role".into()], ..Default::default() };
    assert_eq!(prepare(&c, Write::CreatePost { group: G.into(), draft: odd, of: String::new() }), failed(Failure::Api(GroupFailure::Invalid)));
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

/// What the last read before a write's request is of.
fn last_read(kind: Kind) -> String {
    match kind {
        Kind::DeletePost => "/posts?".into(),
        Kind::AssignRole | Kind::UnassignRole => "/members/".into(),
        Kind::UploadGallery | Kind::UploadIcon => "/auth/user".into(),
        _ => format!("/groups/{G}?"),
    }
}
/// Another account is installed by the service, the way the account thread does, and not published yet (the caller holds the
/// publishing); back once the epoch moved.
fn install_another(c: &Shared) {
    let (before, c2) = (c.identity_epoch(), c.clone());
    std::thread::spawn(move || c2.sign_in_for_test("another"));
    let start = Instant::now();
    while c.identity_epoch() == before && start.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_ne!(c.identity_epoch(), before);
}

/// verify4 F13, for every write: the service installs another account (its publication still to come) before the commit, or
/// while the last read is answered: nothing goes as the one before, and it is known that nothing went.
#[test]
fn nothing_goes_once_another_account_is_installed() {
    for kind in Kind::ALL {
        for during in [false, true] {
            let (c, w, sent) = world();
            let s = prepare(&c, the(&c, kind)).unwrap();
            let (a, key) = (scope(&c), c.groups_key(&scope(&c)).unwrap());
            let turn = c.publishing.lock().unwrap();
            if during {
                let cc = c.clone();
                w.lock().unwrap().then.push((last_read(kind), Box::new(move |_| install_another(&cc))));
            } else {
                install_another(&c);
            }
            let reply = c.groups(LINK, a.clone(), Request::Commit { op: s.op.clone(), digest: s.digest() });
            let private = c.groups(LINK, a, Request::Posts { group: G.into(), offset: 0 });
            drop(turn);
            let refused = [Reply::Failed(Failure::AccountChanged), Reply::Outcome { op: s.op.clone(), outcome: Outcome::Unknown }, Reply::Outcome { op: s.op.clone(), outcome: Outcome::Refused(Failure::AccountChanged) }];
            assert!(refused.contains(&reply), "{kind:?}, {during}: {reply:?}");
            assert_eq!(private, Reply::Failed(Failure::AccountChanged), "nor is anything read for it");
            assert!(writes(&sent).is_empty(), "{kind:?}, during the last read: {during}: {:?}", writes(&sent));
            let e = c.groups_book().get(&key, &s.op).unwrap();
            assert!(matches!((e.state, e.by), (State::Prepared, None) | (State::Refused, Some(By::BeforeSending | By::NeverSent))), "{kind:?}, {during}: {e:?}");
        }
    }
}

/// verify4 F13: what the network has is the account's it was sent by: another one is installed once it is answered, not
/// before; it keeps its outcome for the account it was of.
#[test]
fn an_account_is_installed_after_what_is_under_way() {
    let (c, w, sent) = world();
    let s = prepare(&c, the(&c, Kind::DeletePost)).unwrap();
    let (a, key, hold) = (scope(&c), c.groups_key(&scope(&c)).unwrap(), Arc::new(Hold::default()));
    w.lock().unwrap().fault = Some(Fault::Held(hold.clone()));
    let (c2, op, digest) = (c.clone(), s.op.clone(), s.digest());
    let sending = std::thread::spawn(move || c2.groups(LINK, a, Request::Commit { op, digest }));
    assert!(hold.until_reached());
    let (before, c2) = (c.identity_epoch(), c.clone());
    let installing = std::thread::spawn(move || c2.sign_in_for_test("another"));
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(c.identity_epoch(), before, "waits for its answer");
    assert_eq!(writes(&sent).len(), 1);
    hold.let_go();
    installing.join().unwrap();
    sending.join().unwrap();
    assert_ne!(c.identity_epoch(), before);
    assert_eq!(c.groups_book().get(&key, &s.op).unwrap().state, State::Done, "A's, done");
}

/// Every write, one precondition changed between its confirmation and its send, at each moment it can change: nothing
/// leaves, and it is known that nothing did.
#[test]
fn one_precondition_changed_and_nothing_leaves() {
    type Change = Arc<dyn Fn(&Shared, &Sealed, &mut World) + Send + Sync>;
    let mut rows = 0;
    for kind in Kind::ALL {
        let member = match kind {
            Kind::AssignRole => Some(U),
            Kind::UnassignRole => Some(V),
            _ => None,
        };
        // (what, when: before the commit, before the last read, during the last read)
        let mut changes: Vec<(&str, [bool; 3], Change)> = vec![
            ("another sign-in", [true, false, true], Arc::new(|c, _, _| c.update(|s| s.vrchat_login.revision += 1))),
            ("another member verified", [true, false, true], Arc::new(|c, _, _| c.update(|s| s.vrchat_login.verified_for.as_mut().unwrap().member = "other".into()))),
            ("the same member, another time of theirs", [true, false, true], Arc::new(|c, _, _| c.update(|s| s.vrchat_login.verified_for.as_mut().unwrap().epoch += 1))),
            ("signed out of VRChat", [true, false, true], Arc::new(|c, _, _| c.update(|s| s.vrchat_login.user = None))),
            ("cancelled", [true, false, false], Arc::new(|c, s, _| drop(ask(c, Request::Cancel { op: s.op.clone() })))),
            ("its operation over", [false, false, true], Arc::new(|c, s, _| c.groups_book().end(&s.op, State::Refused, Some(By::BeforeSending), None, Beside::default()))),
            ("the same already committed", [true, false, false], Arc::new(|c, s, _| c.groups.lock().unwrap().used.push_back(s.op.clone()))),
            ("the rights lost", [true, false, false], Arc::new(|_, _, w| (w.rights, w.tags) = (vec!["group-instance-moderate".into()], vec![]))),
        ];
        if let Some(user) = member {
            changes.push(("another membership", [true, true, false], Arc::new(move |_, _, w| w.members.get_mut(user).unwrap()["id"] = json!("gmem_rejoined"))));
            changes.push(("joined again", [true, true, false], Arc::new(move |_, _, w| w.members.get_mut(user).unwrap()["joinedAt"] = json!("2026-11-23T10:00:00Z"))));
            changes.push(("no time of joining", [true, true, false], Arc::new(move |_, _, w| drop(w.members.get_mut(user).unwrap().as_object_mut().unwrap().remove("joinedAt")))));
            changes.push(("left", [true, true, false], Arc::new(move |_, _, w| drop(w.members.remove(user)))));
        }
        for (what, when, change) in changes {
            for moment in (0..3).filter(|m| when[*m]) {
                let (c, w, sent) = world();
                let s = prepare(&c, the(&c, kind)).unwrap();
                let key = c.groups_key(&scope(&c)).unwrap();
                let used = what == "the same already committed";
                if used {
                    // Once, and again by a second press.
                    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
                    commit(&c, &s);
                }
                let gone = writes(&sent).len();
                match moment {
                    0 => {
                        let mut guard = w.lock().unwrap();
                        let mut own = std::mem::take(&mut *guard);
                        drop(guard);
                        change(&c, &s, &mut own);
                        *w.lock().unwrap() = own;
                    }
                    _ => {
                        let (cc, ss, change) = (c.clone(), s.clone(), change.clone());
                        let at = if moment == 1 { format!("/groups/{G}?") } else { last_read(kind) };
                        w.lock().unwrap().then.push((at, Box::new(move |w| change(&cc, &ss, w))));
                    }
                }
                let reply = ask(&c, Request::Commit { op: s.op.clone(), digest: s.digest() });
                assert!(!matches!(&reply, Reply::Outcome { outcome: Outcome::Done(_), .. }), "{kind:?}, {what}, {moment}: {reply:?}");
                assert_eq!(writes(&sent).len(), gone, "{kind:?}, {what}, {moment}: {:?}", writes(&sent));
                if !used {
                    let e = c.groups_book().get(&key, &s.op).unwrap();
                    assert!(!e.open() && !e.stands(), "{kind:?}, {what}, {moment}: it is known that nothing went: {e:?}");
                }
                rows += 1;
            }
        }
    }
    assert!(rows >= 110, "{rows} rows");
}

/// verify4 F1: a press on a row is for that row's membership: the person left and joined again before it was prepared (both
/// its parts another), or VRChat does not say when they joined: not even a confirmation comes of it.
#[test]
fn a_press_is_for_the_rows_own_membership() {
    type Change = fn(&mut Value);
    let changes: [(&str, Change, Failure); 4] = [
        (
            "left and joined again",
            |m| {
                m["id"] = json!("gmem_rejoined");
                m["joinedAt"] = json!("2026-11-23T10:00:00Z");
            },
            Failure::Rejoined,
        ),
        ("another id", |m| m["id"] = json!("gmem_rejoined"), Failure::Rejoined),
        ("another time", |m| m["joinedAt"] = json!("2026-11-23T10:00:00Z"), Failure::Rejoined),
        ("no time", |m| drop(m.as_object_mut().unwrap().remove("joinedAt")), Failure::NotRead(Missing::Member)),
    ];
    for (what, change, want) in changes {
        for (user, role, add) in [(U, "grol_artist", true), (U, "grol_mod", true), (V, "grol_mod", false)] {
            let (c, w, sent) = world();
            let press = Write::MemberRole { group: G.into(), user: user.into(), membership: of(&c, user), role: role.into(), add };
            change(w.lock().unwrap().members.get_mut(user).unwrap());
            assert_eq!(prepare(&c, press), failed(want), "{what}, {role}");
            assert!(writes(&sent).is_empty());
        }
    }
    // Looked at again, a new press is for the new one.
    let (c, w, sent) = world();
    w.lock().unwrap().members.get_mut(U).unwrap()["id"] = json!("gmem_rejoined");
    let press = Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: "grol_artist".into(), add: true };
    let s = prepare(&c, press).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    assert_eq!(writes(&sent).len(), 1);
}

/// REQUEST28: a group from before VRChat's newer flags, as it comes: its first roles without them, one even without its
/// rights, no role flagged as Everyone's. Its owner gives and takes each, with its confirmation; a member who may assign gives
/// those below their own; and one deleted is certified by the list that is left.
#[test]
fn an_old_groups_roles_are_given_by_its_owner() {
    let old = |w: &mut World| {
        for r in w.roles.iter_mut() {
            let r = r.as_object_mut().unwrap();
            for gone in ["defaultRole", "requiresPurchase", "isAddedOnJoin"] {
                r.remove(gone);
            }
        }
        w.roles[1].as_object_mut().unwrap().remove("permissions");
    };
    let give = |c: &Shared, role: &str, add: bool, user: &str| Write::MemberRole { group: G.into(), user: user.into(), membership: of(c, user), role: role.into(), add };
    for (role, add, user) in [("grol_mod", true, U), ("grol_mod", false, V), ("grol_artist", true, U)] {
        let (c, w, sent) = world();
        old(&mut w.lock().unwrap());
        let s = prepare(&c, give(&c, role, add, user)).unwrap_or_else(|r| panic!("{role}: {r:?}"));
        assert_eq!(s.confirm, role == "grol_mod", "{role}: what it may do is not known, or it may do nothing");
        assert!(matches!(commit(&c, &s), Outcome::Done(_)), "{role}");
        assert_eq!(writes(&sent), [format!("{} /groups/{G}/members/{user}/roles/{role}", if add { "PUT" } else { "DELETE" })]);
    }
    // What the group's answer leaves out is read once from the roles' own list, before the confirmation shows it.
    let (c, w, sent) = world();
    let whole = w.lock().unwrap().roles.clone();
    old(&mut w.lock().unwrap());
    w.lock().unwrap().list = Some(whole);
    let s = prepare(&c, give(&c, "grol_mod", true, U)).unwrap();
    assert_eq!(s.role.as_ref().unwrap().permissions, ["group-instance-moderate"]);
    assert!(super::world::calls(&sent).iter().any(|c| c.ends_with("/roles")));
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    // Not the owner, with the right to assign and nothing else: below their own role, by its rank.
    let (c, w, sent) = world();
    {
        let mut w = w.lock().unwrap();
        old(&mut w);
        w.rights = vec!["group-roles-assign".into()];
        w.words = json!({"ownerId": "usr_else"});
        let mut g = w.group();
        g["myMember"]["roleIds"] = json!(["grol_artist"]);
        w.instead = vec![(format!("/groups/{G}?"), g)];
    }
    assert_eq!(prepare(&c, give(&c, "grol_mod", true, U)), failed(Failure::Withheld(Why::RoleAtOrAbove)));
    let s = prepare(&c, give(&c, "grol_events", true, U)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    assert_eq!(writes(&sent).len(), 1);
    // A role is deleted, and the list that is left says so, with no role flagged as Everyone's in it.
    let (c, w, _) = world();
    {
        let mut w = w.lock().unwrap();
        old(&mut w);
        w.roles[1]["permissions"] = json!([]);
    }
    let s = prepare(&c, Write::DeleteRole { group: G.into(), role: "grol_events".into() }).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
}

/// REQUEST28: a group's calendar is read through the service, for the account it was asked as, and never written.
#[test]
fn a_groups_events_are_read_only() {
    let (c, w, sent) = world();
    let event = |id: &str| json!({"id": id, "ownerId": G, "title": "Showcase", "startsAt": "2026-11-28T20:00:00Z", "endsAt": "2026-11-28T22:00:00Z", "accessType": "public", "category": "arts", "description": ""});
    w.lock().unwrap().instead = vec![("/calendar/".into(), json!({"results": [event("cal_1"), event("cal_2")], "hasNext": false, "totalCount": 2}))];
    let month = orbiters_core::group_events::Month { year: 2026, month: 11 };
    let Reply::Events(page) = ask(&c, Request::Events { group: G.into(), month: Some(month), offset: 0 }) else { panic!() };
    assert_eq!((page.items.len(), page.more), (2, false));
    assert_eq!(ask(&c, Request::Events { group: "grp_/../x".into(), month: None, offset: 0 }), Reply::Failed(Failure::Api(GroupFailure::NotFound)));
    assert!(writes(&sent).is_empty() && super::world::calls(&sent).iter().any(|c| c.contains(&format!("/calendar/{G}?n=100&offset=0&date=2026-11-15"))));
}

/// verify4 F3: a role read without its name or a flag is not deleted; a list that is not the group's whole list certifies no
/// deletion, neither as its answer nor at a check; the role still in it says it is there.
#[test]
fn a_deletion_is_certified_by_the_whole_list_only() {
    for (field, odd) in [("name", None), ("permissions", None), ("order", None), ("description", Some(json!(7))), ("requiresTwoFactor", Some(json!("no")))] {
        let (c, w, sent) = world();
        match odd {
            Some(v) => w.lock().unwrap().roles[3][field] = v,
            None => drop(w.lock().unwrap().roles[3].as_object_mut().unwrap().remove(field)),
        }
        assert_eq!(prepare(&c, Write::DeleteRole { group: G.into(), role: "grol_events".into() }), failed(Failure::NotRead(Missing::RoleDefinition)), "{field}");
        assert!(writes(&sent).is_empty());
    }
    let full = World::default().roles;
    let without = |ids: &[&str]| -> Vec<Value> { full.iter().filter(|r| !ids.contains(&r["id"].as_str().unwrap())).cloned().collect() };
    let mut unranked = without(&["grol_events"]);
    unranked[1].as_object_mut().unwrap().remove("order");
    let lists = [
        ("everyone's alone, bare", vec![json!({"id": "grol_everyone", "defaultRole": true})]),
        ("everyone's alone", vec![full[4].clone()]),
        ("no owner's", without(&["grol_owner", "grol_events"])),
        ("no everyone's", without(&["grol_everyone", "grol_events"])),
        ("another gone with it", without(&["grol_artist", "grol_events"])),
        ("one not whole enough to rank", unranked),
    ];
    for (what, list) in lists {
        let (c, w, _) = world();
        let s = prepare(&c, the(&c, Kind::DeleteRole)).unwrap();
        w.lock().unwrap().fault = Some(Fault::Answer { done: true, status: 200, body: json!(list) });
        assert_eq!(commit(&c, &s), Outcome::Unknown, "{what}: as its answer");
        let (c, w, _) = world();
        let op = lost(&c, &w, Kind::DeleteRole, false);
        later(&c);
        let mut g = w.lock().unwrap().group();
        g["roles"] = json!(list);
        w.lock().unwrap().instead = vec![(format!("/groups/{G}?"), g)];
        assert_eq!(check(&c, &op), Observation::CannotTell(Unclear::NotRead), "{what}: at a check");
        assert!(entry(&c, &op).open());
    }
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::DeleteRole, false);
    later(&c);
    assert_eq!(check(&c, &op), Observation::ShowsNot, "still there");
}

/// verify4 F4: what was settled before is said as that, with nothing read; a check whose result cannot be kept says so, and
/// what it is of stays as it was.
#[test]
fn a_settled_check_is_not_said_as_now() {
    let (c, w, sent) = world();
    let op = lost(&c, &w, Kind::AssignRole, true);
    assert_eq!(check(&c, &op), Observation::Shows);
    w.lock().unwrap().members.get_mut(U).unwrap()["roleIds"] = json!([]);
    let reads = sent.lock().unwrap().len();
    assert_eq!(check(&c, &op), Observation::Closed, "never as what VRChat shows now");
    assert_eq!(sent.lock().unwrap().len(), reads);
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::AssignRole, true);
    c.groups_book().full.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(ask(&c, Request::Check { op: op.clone() }), Reply::Failed(Failure::LookNotKept));
    c.groups_book().full.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(entry(&c, &op).open(), "it may have been done, as before");
    assert_eq!(check(&c, &op), Observation::Shows);
}

/// verify4 F12: the rights' catalogue failing, or null, blocks neither the page nor what needs none of it; a new role with
/// rights needs it.
#[test]
fn the_catalogue_blocks_only_what_needs_it() {
    for answer in [Value::Null, json!({"error": "x"})] {
        let (c, w, _) = world();
        w.lock().unwrap().instead = vec![("/permissions".into(), answer)];
        let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
        assert!(page.catalogue.is_none());
        for kind in [Kind::EditRole, Kind::DeleteRole, Kind::AssignRole] {
            assert!(prepare(&c, the(&c, kind)).is_ok(), "{kind:?}");
        }
        let write = the(&c, Kind::CreateRole);
        assert_eq!(prepare(&c, write), failed(Failure::NotRead(Missing::Catalogue)));
    }
}

/// What can be put back of `op`, and what the record says moved.
fn aftermath(c: &Shared, op: &str) -> (Beside, Vec<Write>) {
    match ask(c, Request::Ledger { also: vec![] }) {
        Reply::Ledger { entries, back, .. } => (entries.iter().find(|e| e.op == op).map(|e| e.beside.clone()).unwrap_or_default(), back.into_iter().filter(|(o, _)| o == op).map(|(_, w)| w).collect()),
        other => panic!("{other:?}"),
    }
}

/// The sparse updates against a VRChat that clears what is left out: the banner goes in, the icon, the nameplate and the words
/// go with it: said part by part, and the icon is put back as a write of its own, confirmed, with the picture that was there
/// (whoever's it is); the nameplate is not (it is never sent), nor the words.
#[test]
fn what_a_sparse_update_cleared_is_said_and_put_back() {
    let (c, w, sent) = world();
    w.lock().unwrap().icon = json!("file_someones");
    w.lock().unwrap().words = json!({"nameplateId": "file_plate", "description": "Who we are", "rules": "Be kind", "links": ["https://example.org"]});
    w.lock().unwrap().clears = true;
    let s = prepare(&c, the(&c, Kind::ApplyBanner)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(Done::Group(_))));
    let (beside, back) = aftermath(&c, &s.op);
    assert_eq!(beside.moved, [Part::Icon, Part::Nameplate, Part::Description, Part::Rules, Part::Links]);
    assert_eq!(back, [Write::ApplyArt { group: G.into(), slot: ArtSlot::Icon, file: "file_someones".into(), back: Some(s.op.clone()) }]);
    // Only what the service itself read there, for that slot, of that write.
    for (slot, file, op) in [(ArtSlot::Icon, "file_elses", s.op.as_str()), (ArtSlot::Banner, "file_someones", s.op.as_str()), (ArtSlot::Icon, "file_someones", "another")] {
        assert_eq!(prepare(&c, Write::ApplyArt { group: G.into(), slot, file: file.into(), back: Some(op.into()) }), failed(Failure::Expired));
    }
    let r = prepare(&c, back[0].clone()).unwrap();
    assert!(r.confirm, "confirmed as any");
    let Reply::Picture(shown) = ask(&c, Request::SealedArt { op: r.op.clone() }) else { panic!() };
    assert_eq!((shown.as_slice(), r.art.as_ref().unwrap().sha256.as_str()), (super::world::picture("file_someones").as_slice(), sha256(&shown).as_str()), "the picture that was there");
    // VRChat clears again: that is said too, with the banner to put back.
    assert!(matches!(commit(&c, &r), Outcome::Done(Done::Group(_))));
    let puts: Vec<Value> = sent.lock().unwrap().iter().filter(|r| r.method == "PUT").map(|r| serde_json::from_str(r.body.as_deref().unwrap()).unwrap()).collect();
    assert_eq!(puts, [json!({"bannerId": "file_gal1"}), json!({"iconId": "file_someones"})], "each with its own id alone, and no nameplate ever");
    let (first, _) = aftermath(&c, &s.op);
    assert_eq!(first.moved, [Part::Nameplate, Part::Description, Part::Rules, Part::Links], "the icon is put back: the rest is still to be seen to");
    let (second, again) = aftermath(&c, &r.op);
    assert_eq!((second.moved, again.len()), (vec![Part::Banner], 1));
    // One that does not clear: put back, nothing more to say.
    w.lock().unwrap().clears = false;
    let b = prepare(&c, again[0].clone()).unwrap();
    assert!(matches!(commit(&c, &b), Outcome::Done(_)));
    assert_eq!(aftermath(&c, &r.op), (Beside::default(), vec![]));
    assert_eq!(w.lock().unwrap().banner, json!("file_gal1"));
    // Seen to by the person.
    assert_eq!(ask(&c, Request::SeenTo { op: s.op.clone() }), Reply::Decided { op: s.op.clone() });
    assert_eq!(aftermath(&c, &s.op).0, Beside::default());
}

/// The same of a role: its rights went with a rename, and a flag and its words: the flag and the words are put back, with
/// nothing else in the request; its rights are not (they are changed on vrchat.com).
#[test]
fn what_a_renames_update_cleared_of_a_role_is_said_and_put_back() {
    for dropped in [false, true] {
        let (c, w, sent) = world();
        {
            let mut w = w.lock().unwrap();
            (w.roles[2]["permissions"], w.roles[2]["isSelfAssignable"], w.roles[2]["description"]) = (json!(["group-instance-join"]), json!(true), json!("They paint"));
            w.clears = true;
        }
        let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
        let baseline = page.group.roles.unwrap().into_iter().find(|r| r.id == "grol_artist").unwrap();
        let edit = RoleEdit { name: "Painters".into(), ..RoleEdit::of(&baseline) };
        let s = prepare(&c, Write::EditRole { group: G.into(), baseline, edit, back: None }).unwrap();
        if dropped {
            // Its answer lost: the check sees the same, and what was there is still held.
            w.lock().unwrap().fault = Some(Fault::Drop { done: true });
            assert_eq!(commit(&c, &s), Outcome::Unknown);
            assert_eq!(check(&c, &s.op), Observation::Shows);
        } else {
            assert!(matches!(commit(&c, &s), Outcome::Done(Done::Roles(_))));
        }
        let (beside, back) = aftermath(&c, &s.op);
        assert_eq!(beside.moved, [Part::Description, Part::Rights, Part::SelfAssignable], "{dropped}");
        assert_eq!(back.len(), 1);
        w.lock().unwrap().clears = false;
        let r = prepare(&c, back[0].clone()).unwrap();
        assert!(r.confirm);
        assert!(matches!(commit(&c, &r), Outcome::Done(_)));
        let last = sent.lock().unwrap().iter().rfind(|r| r.method == "PUT").map(|r| serde_json::from_str::<Value>(r.body.as_deref().unwrap()).unwrap());
        assert_eq!(last, Some(json!({"description": "They paint", "isSelfAssignable": true})), "what can be put back from here, and nothing else");
        assert_eq!(aftermath(&c, &s.op), (Beside { moved: vec![Part::Rights], unread: vec![] }, vec![]), "its rights are still to be seen to, on vrchat.com");
    }
}

/// Put back is confirmed whatever it is of: the words alone of a role with no rights, which change with no confirmation when
/// they are the person's own edit.
#[test]
fn a_put_back_is_always_confirmed() {
    let (c, w, _) = world();
    {
        let mut w = w.lock().unwrap();
        (w.roles[2]["description"], w.clears) = (json!("They paint"), true);
    }
    let edit_of = |name: &str| {
        let Reply::Opened(page) = ask(&c, Request::Open { group: G.into() }) else { panic!() };
        let baseline = page.group.roles.unwrap().into_iter().find(|r| r.id == "grol_artist").unwrap();
        let edit = RoleEdit { name: name.into(), ..RoleEdit::of(&baseline) };
        prepare(&c, Write::EditRole { group: G.into(), baseline, edit, back: None }).unwrap()
    };
    let s = edit_of("Painters");
    assert!(!s.confirm, "the person's own edit of its name");
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    let (beside, back) = aftermath(&c, &s.op);
    assert_eq!((beside.moved, back.len()), (vec![Part::Description], 1));
    assert!(prepare(&c, back[0].clone()).unwrap().confirm);
}

/// verify4 F7: each part is compared on its own, whatever the answer leaves out of the others; what it leaves out is read
/// again; what that leaves out too is said as not known.
#[test]
fn what_an_answer_leaves_out_hides_nothing() {
    let words = json!({"description": "Who we are", "rules": "Be kind", "links": ["https://example.org"], "joinState": "open"});
    // What VRChat has changes with the write (right after the last read before it).
    let apply = |change: fn(&mut Value, &mut Value)| {
        let (c, w, _) = world();
        w.lock().unwrap().words = words.clone();
        let s = prepare(&c, the(&c, Kind::ApplyBanner)).unwrap();
        let mut after = w.lock().unwrap().group();
        after["bannerId"] = json!("file_gal1");
        let mut there = words.clone();
        change(&mut after, &mut there);
        {
            let mut w = w.lock().unwrap();
            w.fault = Some(Fault::Answer { done: true, status: 200, body: after });
            w.then.push((format!("/groups/{G}?"), Box::new(move |w| w.words = there)));
        }
        assert!(matches!(commit(&c, &s), Outcome::Done(_)));
        entry(&c, &s.op).beside
    };
    assert_eq!(apply(|_, _| {}), Beside::default());
    let cleared = apply(|a, w| {
        a["description"] = json!("");
        a.as_object_mut().unwrap().remove("links");
        w["description"] = json!("");
    });
    assert_eq!(cleared, Beside { moved: vec![Part::Description], unread: vec![] }, "the links read again: the same");
    let answered = apply(|a, _| {
        a["description"] = json!("");
        a.as_object_mut().unwrap().remove("links");
    });
    assert_eq!(answered, Beside { moved: vec![Part::Description], unread: vec![] }, "its answer alone says so: said all the same");
    let links = apply(|a, w| {
        a.as_object_mut().unwrap().remove("links");
        w["links"] = json!([]);
    });
    assert_eq!(links, Beside { moved: vec![Part::Links], unread: vec![] }, "gone, by the read");
    let never = apply(|a, w| {
        a.as_object_mut().unwrap().remove("rules");
        w.as_object_mut().unwrap().remove("rules");
        a["joinState"] = json!("closed");
        w["joinState"] = json!("closed");
    });
    assert_eq!(never, Beside { moved: vec![Part::JoinState], unread: vec![Part::Rules] }, "not said is not known");
}

/// The record's entries are the scope's own Orbiters member's, by the id they were verified for: the same at another time
/// of theirs, another's under the same name, and never of whoever is signed in by now.
#[test]
fn the_records_owner_is_the_scopes() {
    let (c, _, _) = world();
    let verified = |epoch: u64, member: &str| {
        let v = orbiters_core::launcher::VerifiedFor { epoch, member: member.into(), user: Some("usr_test".into()) };
        c.update(|s| s.vrchat_login.verified_for = Some(v));
        scope(&c)
    };
    let (a, again, b) = (verified(1, "a"), verified(2, "a"), verified(3, "b"));
    let key = |s: &Scope| c.groups_key(s).unwrap();
    assert_eq!((key(&a), key(&a).orbiters.as_str()), (key(&again), "a"));
    assert_ne!(key(&a), key(&b));
    assert!(a != again, "another time of theirs is another scope");
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
    let e = c.groups_book().get(&c.groups_key(&scope(&c)).unwrap(), &s.op).unwrap();
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
    assert_eq!(check(&c, &s.op), Observation::Answered { done: true }, "the durable scope is the same account's");
}

/// verify3, "Scope": the account, its sign-in or the environment is looked at after the last read too: what changed while
/// it was read stops the request.
#[test]
fn the_scope_is_looked_at_again_at_the_send() {
    for kind in Kind::ALL {
        let (c, w, sent) = world();
        let s = prepare(&c, the(&c, kind)).unwrap();
        let last = match kind {
            Kind::DeletePost => "/posts?".to_string(),
            Kind::AssignRole | Kind::UnassignRole => "/members/".into(),
            Kind::UploadGallery | Kind::UploadIcon => "/auth/user".into(),
            _ => format!("/groups/{G}?"),
        };
        let cc = c.clone();
        // At the last read of all (the first of the group's, where two are made).
        w.lock().unwrap().then.push((last, Box::new(move |_| cc.update(|s| s.vrchat_login.revision += 1))));
        let Reply::Outcome { outcome, .. } = ask(&c, Request::Commit { op: s.op.clone(), digest: s.digest() }) else { panic!() };
        assert!(!matches!(outcome, Outcome::Done(_)), "{kind:?}: {outcome:?}");
        assert!(writes(&sent).is_empty(), "{kind:?}: nothing goes as the sign-in that ended");
        let e = c.groups_book().of(&c.groups_key(&scope(&c)).unwrap()).into_iter().find(|e| e.op == s.op).unwrap();
        assert_eq!(e.state, State::Refused, "{kind:?}: and it is known that nothing went");
    }
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
    let s = prepare(&c, the(&c, Kind::DeletePost)).unwrap();
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
    let mut target = write.ids();
    if target.user.is_some() {
        target.membership = Some("m".into());
    }
    book.prepare(Entry { v: FORMAT, op: op.clone(), key: c.groups_key(&scope(c)).unwrap(), group: write.group().into(), kind: write.kind(), target, payload: write.payload(), same: write.same(), evidence, state: State::Prepared, by: None, made: None, checked: false, answer: None, looked: false, closed: None, beside: Beside::default(), prepared_at: book.now(), dispatched_at: None, ended_at: None }).unwrap();
    book.dispatch(&op, &[]).unwrap();
    book.end(&op, State::Unknown, None, None, Beside::default());
    later(c);
    op
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
    let op = unknown_entry(&c, &Write::ApplyArt { group: G.into(), slot: ArtSlot::Icon, file: "file_new".into(), back: None }, Evidence::default());
    assert_eq!(check(&c, &op), Observation::CannotTell(Unclear::NotRead));
}
#[test]
fn verify2_disputed_member_must_be_the_requested_user() {
    let (c, w, _) = world();
    w.lock().unwrap().instead = vec![(format!("/members/{U}"), super::world::member(TWIN, &["grol_artist"]))];
    let op = unknown_entry(&c, &Write::MemberRole { group: G.into(), user: U.into(), membership: of(&c, U), role: "grol_artist".into(), add: true }, Evidence::default());
    assert_eq!(check(&c, &op), Observation::CannotTell(Unclear::NotRead));
}

// --- the third verification's counterexamples (verify3-groups-scratch/probes.rs), as what should happen -----------------

#[test]
fn review3_b1_role_membership_changed_after_seal() {
    let (c, w, sent) = world();
    let s = prepare(&c, the(&c, Kind::AssignRole)).unwrap();
    w.lock().unwrap().members.get_mut(U).unwrap()["id"] = json!("gmem_rejoined");
    assert!(matches!(commit(&c, &s), Outcome::Refused(_)), "this person left and rejoined");
    assert!(writes(&sent).is_empty());
}
#[test]
fn review3_b1_rejoined_membership() {
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::AssignRole, false);
    let mut rejoined = super::world::member(U, &["grol_mod"]);
    rejoined["id"] = json!("gmem_new_membership");
    w.lock().unwrap().members.insert(U.into(), rejoined);
    later(&c);
    let _ = check(&c, &op);
    assert!(entry(&c, &op).open(), "another membership's role says nothing of this one's");
}
#[test]
fn review3_b2_role_reverted_after_success() {
    let (c, w, _) = world();
    let before = w.lock().unwrap().roles.clone();
    let op = lost(&c, &w, Kind::EditRole, true);
    w.lock().unwrap().roles = before;
    later(&c);
    let _ = check(&c, &op);
    assert_ne!(entry(&c, &op).state, State::Refused, "it was done and undone: never 'it was not done'");
    assert!(entry(&c, &op).open(), "the person's to decide");
}
#[test]
fn review3_b2_unrelated_identical_role() {
    let (c, w, _) = world();
    let op = lost(&c, &w, Kind::CreateRole, false);
    let mut r = role("grol_another", "Greeters", 6, &["group-instance-join"], false);
    r["description"] = json!("Say hello");
    r["createdAt"] = json!(super::world::now());
    w.lock().unwrap().roles.push(r);
    later(&c);
    let _ = check(&c, &op);
    assert_ne!(entry(&c, &op).state, State::Done, "another moderator's role is not this creation");
    assert!(entry(&c, &op).open());
}
#[test]
fn review3_b2_upload_before_dispatch() {
    let (c, w, _) = world();
    let write = the(&c, Kind::UploadGallery);
    let Write::Upload { staged, .. } = &write else { panic!() };
    let bytes = staged.read().unwrap();
    let sealed = prepare(&c, write).unwrap();
    let mut f = super::world::file("file_other", "gallery", "usr_test", &bytes);
    f["versions"][1]["created_at"] = json!("2025-01-01T00:00:00Z");
    w.lock().unwrap().files.push(("gallery".into(), f));
    w.lock().unwrap().fault = Some(Fault::Drop { done: false });
    assert_eq!(commit(&c, &sealed), Outcome::Unknown);
    let _ = check(&c, &sealed.op);
    assert_ne!(entry(&c, &sealed.op).state, State::Done, "a file that was there before the send is not this upload");
    assert!(entry(&c, &sealed.op).open());
}
#[test]
fn review3_b3_two_prepared_windows() {
    let (c, _, sent) = world();
    let a = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    let Reply::Prepared(b) = c.groups(LINK + 1, scope(&c), Request::Prepare { write: a.write.clone() }) else { panic!() };
    assert_eq!(b.warning, None);
    assert!(matches!(commit(&c, &a), Outcome::Done(_)));
    let Reply::Outcome { outcome, .. } = c.groups(LINK + 1, scope(&c), Request::Commit { op: b.op.clone(), digest: b.digest() }) else { panic!() };
    assert!(matches!(outcome, Outcome::Refused(_)), "the second was shown nothing of the first: {outcome:?}");
    assert_eq!(writes(&sent).len(), 1);
}
/// Blocker 4: its edit has left this version (`no_post_is_ever_edited_from_here`); a restriction put between the last read
/// and the request of what is left, a deletion or a new post, is never undone by them.
#[test]
fn review3_b4_just_restricted_audience() {
    let (c, w, _) = world();
    let s = prepare(&c, the(&c, Kind::DeletePost)).unwrap();
    w.lock().unwrap().then.push(("/posts?".into(), Box::new(|w| w.posts[0]["roleIds"] = json!(["grol_events"]))));
    let _ = commit(&c, &s);
    assert_eq!(w.lock().unwrap().posts[0]["roleIds"], json!(["grol_events"]), "the other moderator's restriction stays");
    let (c, w, _) = world();
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    w.lock().unwrap().then.push((format!("/groups/{G}?"), Box::new(|w| w.posts[0]["roleIds"] = json!(["grol_events"]))));
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    assert!(w.lock().unwrap().posts.iter().any(|p| p["id"] == "gpos_1" && p["roleIds"] == json!(["grol_events"])));
}
#[test]
fn review3_equal_accounts_two_ledger_owners() {
    let (a, _, sa) = world();
    let (b, _, sb) = world();
    b.groups_keep_book_for_tests(a.groups_book().dir());
    let one = prepare(&a, the(&a, Kind::CreatePost));
    assert_eq!(ask(&b, Request::Ledger { also: vec![] }), Reply::Failed(Failure::OtherOwner));
    let two = prepare(&b, the(&b, Kind::CreatePost));
    assert_eq!(two, failed(Failure::OtherOwner));
    for (c, s) in [(&a, one), (&b, two)] {
        if let Ok(s) = s {
            let _ = commit(c, &s);
        }
    }
    assert_eq!(writes(&sa).len() + writes(&sb).len(), 1, "one owner of the record at a time");
}
#[test]
fn review3_unreadable_completed_record() {
    let (c, _, _) = world();
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    assert!(matches!(commit(&c, &s), Outcome::Done(_)));
    let dir = c.groups_book().dir();
    c.groups_book().let_go();
    let whole = std::fs::read(dir.join(format!("{}.json", s.op))).unwrap();
    std::fs::write(dir.join(format!("{}.json", s.op)), &whole[..whole.len() / 2]).unwrap();
    let (b, _, sent) = world();
    b.groups_keep_book_for_tests(dir);
    assert_eq!(check(&b, &s.op), Observation::CannotTell(Unclear::NoRecord), "a record that cannot be read is not 'never sent'");
    assert_eq!(prepare(&b, the(&b, Kind::CreatePost)), failed(Failure::RecordUnreadable), "not sent past it unseen");
    let Reply::Ledger { unreadable, .. } = ask(&b, Request::Ledger { also: vec![] }) else { panic!() };
    assert_eq!(unreadable, 1, "and the window is told");
    assert_eq!(ask(&b, Request::SetAside), Reply::Cancelled);
    let again = prepare(&b, the(&b, Kind::CreatePost)).unwrap();
    assert_eq!((again.warning, again.confirm), (Some(Warning::RecordUnreadable), true));
    assert!(writes(&sent).is_empty());
}
/// Only its own newly started test program is ended, at each side of the request: the next program on the same record
/// finds it unknown, sends nothing by itself and refuses the same.
#[test]
fn review3_real_process_kills() {
    for phase in ["pre", "post"] {
        let root = orbiters_core::paths::local_data_root().join("group-kills").join(uuid::Uuid::new_v4().to_string()).join(phase);
        std::fs::create_dir_all(&root).unwrap();
        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(exe).args(["--exact", "controller::groups::tests::review3_kill_helper"]).env("GROUP_REVIEW3_KILL", &root).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let start = Instant::now();
        while !root.join("ready").exists() && start.elapsed() < Duration::from_secs(60) && child.try_wait().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ready = root.join("ready").exists();
        let _ = child.kill();
        child.wait().unwrap();
        assert!(ready, "{phase}");
        let (c, _, sent) = world();
        c.groups_keep_book_for_tests(root.join("book"));
        let op = std::fs::read_to_string(root.join("op")).unwrap();
        assert_eq!(entry(&c, &op).state, State::Unknown, "{phase}: its lock ended with it");
        assert_eq!(prepare(&c, the(&c, Kind::CreatePost)), Err(Reply::MaySent { op }), "{phase}");
        assert!(writes(&sent).is_empty(), "{phase}");
    }
}
#[test]
fn review3_kill_helper() {
    let Some(dir) = std::env::var_os("GROUP_REVIEW3_KILL") else { return };
    let dir = std::path::PathBuf::from(dir);
    let (c, w, _) = world();
    c.groups_keep_book_for_tests(dir.join("book"));
    let s = prepare(&c, the(&c, Kind::CreatePost)).unwrap();
    std::fs::write(dir.join("op"), &s.op).unwrap();
    if dir.ends_with("pre") {
        w.lock().unwrap().then.push((
            format!("/groups/{G}?"),
            Box::new(move |_| {
                std::fs::write(dir.join("ready"), "last read").unwrap();
                loop {
                    std::thread::park();
                }
            }),
        ));
    } else {
        let hold = Arc::new(Hold::default());
        w.lock().unwrap().fault = Some(Fault::Held(hold.clone()));
        std::thread::spawn(move || {
            assert!(hold.until_reached());
            std::fs::write(dir.join("ready"), "applied, no answer yet").unwrap();
        });
    }
    let _ = commit(&c, &s);
}
