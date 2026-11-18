//! Recorded-shape replies (made-up people, field names from the spec) for every answer and pipeline message handled.
use super::*;
use serde_json::json;

const A: &str = "usr_00000000-0000-4000-8000-000000000001";
const B: &str = "usr_00000000-0000-4000-8000-000000000002";
const W: &str = "wrld_00000000-0000-4000-8000-0000000000aa";

fn online_friend() -> Value {
    json!({
        "id": A, "displayName": "Nova Sample", "status": "join me", "statusDescription": "come hang",
        "location": format!("{W}:12345~hidden({B})~region(eu)"), "platform": "standalonewindows", "last_platform": "standalonewindows",
        "tags": ["system_trust_veteran", "system_supporter", "language_eng", "language_jpn"],
        "userIcon": "https://api.vrchat.cloud/api/1/file/file_icon/1", "currentAvatarThumbnailImageUrl": "https://api.vrchat.cloud/thumb",
        "bannerUrl": "", "bannerColor": "#2040A0", "iconFrame": "invt_frame", "nameplateEffect": "invt_plate",
        "last_login": "2026-11-17T20:00:00.000Z", "last_activity": "2026-11-18T08:00:00.000Z", "isFriend": true, "friendKey": "x"
    })
}

#[test]
fn a_friend_in_a_world_reads_whole() {
    let f = friend(&online_friend(), true).unwrap();
    assert_eq!(f.name, "Nova Sample");
    assert_eq!((f.status, f.presence, f.platform), (Status::JoinMe, Presence::InGame, Platform::Pc));
    let Whereabouts::World(p) = &f.whereabouts else { panic!("{:?}", f.whereabouts) };
    assert_eq!((p.world_id.as_str(), p.access, p.region.as_str()), (W, InstanceAccess::FriendsPlus, "eu"));
    assert_eq!(f.look.rank, Rank::TrustedUser);
    assert!(f.look.supporter);
    assert_eq!(f.look.icon, "https://api.vrchat.cloud/api/1/file/file_icon/1");
    assert_eq!(f.look.banner_colour, Some([0x20, 0x40, 0xA0]));
    assert_eq!(f.last_seen, seconds("2026-11-18T08:00:00Z"));
    assert_eq!(decorations(&online_friend()), Decorations { icon_frame: Some("invt_frame".into()), nameplate: Some("invt_plate".into()) });
}

#[test]
fn website_private_and_offline_friends_land_in_their_sections() {
    let website = json!({"id": B, "displayName": "Lumen", "status": "active", "location": "offline", "platform": "web", "tags": []});
    let f = friend(&website, true).unwrap();
    assert_eq!((f.presence, f.whereabouts.clone(), f.platform), (Presence::Website, Whereabouts::Website, Platform::Web));
    let private = json!({"id": B, "displayName": "Lumen", "status": "ask me", "location": "private", "platform": "android"});
    assert_eq!(friend(&private, true).unwrap().whereabouts, Whereabouts::Private);
    let offline = json!({"id": B, "displayName": "Lumen", "status": "active", "location": "offline", "platform": "", "last_platform": "android"});
    let f = friend(&offline, false).unwrap();
    assert_eq!((f.presence, f.status, f.platform), (Presence::Offline, Status::Offline, Platform::Android));
    assert!(friend(&json!({"id": "nope"}), true).is_none());
}

#[test]
fn instance_kinds_follow_their_tokens() {
    for (instance, kind) in [
        ("1", InstanceAccess::Public),
        ("1~region(jp)", InstanceAccess::Public),
        ("1~hidden(usr_x)~region(eu)", InstanceAccess::FriendsPlus),
        ("1~friends(usr_x)", InstanceAccess::Friends),
        ("1~private(usr_x)~canRequestInvite~region(use)", InstanceAccess::InvitePlus),
        ("1~private(usr_x)~region(use)~nonce(z)", InstanceAccess::Invite),
        ("1~group(grp_x)~groupAccessType(public)", InstanceAccess::GroupPublic),
        ("1~group(grp_x)~groupAccessType(plus)", InstanceAccess::GroupPlus),
        ("1~group(grp_x)~groupAccessType(members)", InstanceAccess::Group),
    ] {
        assert_eq!(access(instance).0, kind, "{instance}");
    }
    assert_eq!(access("1~region(jp)").1, "jp");
    assert_eq!(access("1").1, "us", "VRChat's default region");
    assert_eq!(whereabouts("traveling"), Whereabouts::Traveling);
    assert_eq!(whereabouts("wrld_bad:1"), Whereabouts::Unknown);
}

#[test]
fn favourite_groups_keep_their_names_and_members() {
    let groups = favourite_groups(&json!([
        {"id": "fvgrp_2", "name": "group_1", "displayName": "noice people", "type": "friend"},
        {"id": "fvgrp_1", "name": "group_0", "displayName": "", "type": "friend"},
        {"id": "fvgrp_3", "name": "worlds1", "displayName": "Worlds", "type": "world"}
    ]));
    assert_eq!(groups, [("group_0".to_string(), "group_0".to_string()), ("group_1".to_string(), "noice people".to_string())]);
    let who = favourites(&json!([
        {"id": "fvrt_1", "type": "friend", "favoriteId": A, "tags": ["group_1"]},
        {"id": "fvrt_2", "type": "world", "favoriteId": W, "tags": ["worlds1"]}
    ]));
    assert_eq!(who.get(A).unwrap(), &["group_1".to_string()]);
    assert_eq!(who.len(), 1);
    let mut state = LauncherState::default();
    let mut a = friend(&online_friend(), true).unwrap();
    a.favourite_groups = who[A].clone();
    let b = friend(&json!({"id": B, "displayName": "aaa", "location": "offline"}), false).unwrap();
    state.friends = vec![a, b];
    regroup(&mut state, &groups);
    let names: Vec<_> = state.groups.iter().map(|g| (g.id.as_str(), g.members.len())).collect();
    assert_eq!(names, [("group_0", 0), ("group_1", 1), ("in-game", 1), ("website", 0), ("offline", 1)]);
}

#[test]
fn invites_requests_and_boops_from_notifications() {
    let v1 = json!({
        "id": "not_1", "type": "invite", "senderUserId": A, "senderUsername": "Nova Sample", "created_at": "2026-11-18T09:00:00.000Z",
        "details": format!("{{\"worldId\":\"{W}:777~friends({A})~region(jp)\",\"worldName\":\"Quiet Café\",\"inviteMessage\":\"come!\"}}"),
        "message": "", "seen": false, "receiverUserId": B
    });
    let i = invite(&v1).unwrap();
    assert_eq!((i.kind, i.message.as_str()), (InviteKind::Invite, "come!"));
    let p = i.place.unwrap();
    assert_eq!((p.world_name.as_str(), p.instance_id.as_str(), p.access), ("Quiet Café", "777~friends(usr_00000000-0000-4000-8000-000000000001)~region(jp)", InstanceAccess::Friends));
    let request = json!({"id": "not_2", "type": "requestInvite", "senderUserId": A, "senderUsername": "Nova", "details": {"platform": "android", "requestMessage": "hi"}, "created_at": "2026-11-18T09:00:00Z"});
    let r = invite(&request).unwrap();
    assert_eq!((r.kind, r.place, r.message.as_str()), (InviteKind::Request, None, "hi"));
    assert!(invite(&json!({"id": "not_3", "type": "invite", "senderUserId": A, "details": "{}"})).is_none(), "an invite with no place is not offered");
    assert!(invite(&json!({"id": "not_4", "type": "friendRequest", "senderUserId": A})).is_none());
    let v2 = json!({"id": "not_5", "type": "boop", "senderUserId": A, "category": "social.boop", "createdAt": "2026-11-18T09:01:00.000Z",
        "details": {"emojiId": "default_heart", "emojiVersion": null, "inventoryItemId": null}, "data": {"boopingUserDisplayName": "Nova Sample"},
        "imageUrl": "https://api.vrchat.cloud/icon"});
    let b = boop(&v2).unwrap();
    assert_eq!((b.from_name.as_str(), b.emoji.as_deref(), b.received_at), ("Nova Sample", Some("default_heart"), seconds("2026-11-18T09:01:00Z").unwrap()));
}

#[test]
fn the_pipeline_keeps_the_list_live() {
    let mut state = LauncherState::default();
    let groups = vec![("group_0".to_string(), "best".to_string())];
    let mut a = friend(&online_friend(), true).unwrap();
    a.favourite_groups = vec!["group_0".into()];
    a.look.nameplate = "https://api.vrchat.cloud/plate".into();
    state.friends = vec![a];
    regroup(&mut state, &groups);
    let message = |kind: &str, content: Value| json!({"type": kind, "content": content.to_string()}).to_string();

    let moved = apply(&mut state, &groups, &message("friend-location", json!({"userId": A, "location": format!("{W}:9~region(us)"), "travelingToLocation": "", "worldId": W, "canRequestInvite": true, "user": {"id": A, "displayName": "Nova Sample", "status": "busy", "statusDescription": "afk", "tags": []}, "world": {"id": W, "name": "Sample Lounge", "thumbnailImageUrl": "https://api.vrchat.cloud/w"}})), 100);
    assert!(moved.changed);
    let f = state.friend(A).unwrap();
    let Whereabouts::World(p) = &f.whereabouts else { panic!() };
    assert_eq!((p.instance_id.as_str(), p.world_name.as_str(), f.status), ("9~region(us)", "Sample Lounge", Status::Busy));
    assert_eq!(f.favourite_groups, ["group_0"], "groups survive an update");
    assert_eq!(f.look.nameplate, "https://api.vrchat.cloud/plate", "resolved decorations survive");

    apply(&mut state, &groups, &message("friend-active", json!({"userId": A, "platform": "web", "user": {"id": A, "displayName": "Nova Sample", "status": "active"}})), 110);
    assert_eq!(state.friend(A).unwrap().presence, Presence::Website);
    assert_eq!(state.groups.iter().find(|g| g.id == "website").unwrap().members, [A]);

    apply(&mut state, &groups, &message("friend-offline", json!({"userId": A, "platform": ""})), 120);
    let f = state.friend(A).unwrap();
    assert_eq!((f.presence, f.last_seen), (Presence::Offline, Some(120)));

    apply(&mut state, &groups, &message("friend-online", json!({"userId": A, "location": "private", "platform": "android", "canRequestInvite": false, "user": {"id": A, "displayName": "Nova Sample", "status": "ask me"}})), 130);
    let f = state.friend(A).unwrap();
    assert_eq!((f.presence, f.whereabouts.clone(), f.platform, f.status), (Presence::InGame, Whereabouts::Private, Platform::Android, Status::AskMe));

    apply(&mut state, &groups, &message("friend-update", json!({"userId": A, "user": {"id": A, "displayName": "Nova Renamed", "status": "ask me", "statusDescription": "new"}})), 140);
    let f = state.friend(A).unwrap();
    assert_eq!((f.name.as_str(), f.status_text.as_str(), f.presence), ("Nova Renamed", "new", Presence::InGame), "an update keeps where they are");

    apply(&mut state, &groups, &message("friend-add", json!({"userId": B, "user": {"id": B, "displayName": "New Friend", "status": "active"}})), 150);
    assert!(state.friend(B).is_some());
    apply(&mut state, &groups, &message("friend-delete", json!({"userId": B})), 160);
    assert!(state.friend(B).is_none());

    let invite = json!({"id": "not_9", "type": "invite", "senderUserId": A, "senderUsername": "Nova", "created_at": "2026-11-18T09:00:00Z", "details": {"worldId": format!("{W}:1~region(eu)"), "worldName": "Lounge"}});
    assert!(apply(&mut state, &groups, &message("notification", invite.clone()), 170).arrived);
    assert!(!apply(&mut state, &groups, &message("notification", invite), 171).arrived, "the same invite once");
    assert_eq!(state.invites[0].from_name, "Nova");
    let boop = json!({"id": "not_10", "type": "boop", "senderUserId": A, "createdAt": "2026-11-18T09:01:00Z", "details": {"emojiId": null}, "data": {"boopingUserDisplayName": "Nova"}});
    assert!(apply(&mut state, &groups, &message("notification-v2", boop), 180).arrived);
    assert_eq!(state.boops[0].emoji, None);
    apply(&mut state, &groups, &json!({"type": "hide-notification", "content": "not_9"}).to_string(), 190);
    assert!(state.invites.is_empty());
    apply(&mut state, &groups, &message("notification-v2-delete", json!({"ids": ["not_10"], "version": 2})), 200);
    assert!(state.boops.is_empty());
    assert_eq!(apply(&mut state, &groups, "not json", 210), Applied::default());
    assert_eq!(apply(&mut state, &groups, &message("economy-update", json!({})), 210), Applied::default());
}

#[test]
fn the_profile_gathers_its_three_answers() {
    let base = friend(&online_friend(), true).unwrap();
    let public = json!({"id": A, "displayName": "Nova Sample", "bio": "hello", "bioLinks": ["https://example.com", "javascript:x"], "pronouns": "they/them",
        "languages": ["eng"], "badges": [{"badgeId": "bdg_1", "badgeName": "Early", "badgeDescription": "d", "badgeImageUrl": "https://assets.vrchat.com/b.png", "hidden": false},
        {"badgeId": "bdg_2", "badgeName": "Hidden", "hidden": true}], "bannerUrl": "https://api.vrchat.cloud/banner", "tags": ["system_trust_veteran"]});
    let user = json!({"id": A, "note": "met at a meetup", "date_joined": "2021-04-01", "last_login": "2026-11-18T10:00:00Z", "profilePicOverride": "https://api.vrchat.cloud/big"});
    let p = profile(base, &public, &user, Some((12, 3)));
    assert_eq!((p.bio.as_str(), p.pronouns.as_str(), p.date_joined.as_str(), p.friend.note.as_str()), ("hello", "they/them", "2021-04-01", "met at a meetup"));
    assert_eq!(p.links, ["https://example.com"]);
    assert_eq!(p.badges.len(), 1);
    assert_eq!((p.mutual_friends, p.mutual_groups), (Some(12), Some(3)));
    assert_eq!(p.friend.look.banner, "https://api.vrchat.cloud/banner");
    assert_eq!(p.picture, "https://api.vrchat.cloud/big");
    assert_eq!(p.languages, ["eng"]);
    let empty = profile(friend(&online_friend(), true).unwrap(), &Value::Null, &Value::Null, None);
    assert!(empty.languages.is_empty() && empty.bio.is_empty(), "nothing made up when the answers are missing");
}

#[test]
fn cooldowns_hold_per_person() {
    let mut c = Cooldowns::default();
    assert_eq!(c.take(true, A, 0), Ok(()));
    assert_eq!(c.take(true, A, 10), Err(Cooldowns::BOOP));
    assert_eq!(c.take(true, B, 10), Ok(()), "another person");
    assert_eq!(c.take(false, A, 10), Ok(()), "a request is its own");
    assert_eq!(c.take(true, A, Cooldowns::BOOP), Ok(()), "allowed again");
    c.release(false, A);
    assert_eq!(c.take(false, A, 11), Ok(()));
    let mut friends = vec![friend(&online_friend(), true).unwrap()];
    c.mark(&mut friends, 70);
    assert_eq!(friends[0].boop_ready_at, Some(2 * Cooldowns::BOOP));
}
