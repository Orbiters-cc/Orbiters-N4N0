//! VRChat's answers and pipeline messages turned into the launcher's shapes ([`crate::launcher`]). Pure: no network, no
//! clock (the time is passed in). Field names follow the community OpenAPI spec (vrchatapi/specification 2026-11-10):
//! `LimitedUserFriend`, `User`, `PublicProfile`, `Notification`, `NotificationV2`, `Websocket*`.
use crate::launcher::*;
use serde_json::Value;
use std::collections::BTreeMap;

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string()
}

/// A picture address VRChat gave: only `https://`.
fn picture(v: &Value, key: &str) -> String {
    Some(text(v, key)).filter(|u| u.starts_with("https://")).unwrap_or_default()
}

fn tags(v: &Value) -> Vec<&str> {
    v.get("tags").and_then(Value::as_array).map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// A JSON value that VRChat sometimes sends as a JSON-encoded string (`details`, a pipeline message's `content`).
pub fn decoded(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

pub fn status(s: &str) -> Status {
    match s {
        "join me" => Status::JoinMe,
        "active" => Status::Active,
        "ask me" => Status::AskMe,
        "busy" => Status::Busy,
        _ => Status::Offline,
    }
}

pub fn platform(s: &str) -> Platform {
    match s {
        "standalonewindows" => Platform::Pc,
        "android" => Platform::Android,
        "ios" => Platform::Ios,
        "web" => Platform::Web,
        _ => Platform::Unknown,
    }
}

/// VRChat's trust tags: `basic` is New User, `known` User, `trusted` Known User, `veteran` Trusted User.
pub fn rank(tags: &[&str]) -> Rank {
    let has = |t: &str| tags.contains(&t);
    if has("system_trust_veteran") {
        Rank::TrustedUser
    } else if has("system_trust_trusted") {
        Rank::KnownUser
    } else if has("system_trust_known") {
        Rank::User
    } else if has("system_trust_basic") {
        Rank::NewUser
    } else {
        Rank::Visitor
    }
}

/// `language_eng` → `eng`.
pub fn languages(tags: &[&str]) -> Vec<String> {
    tags.iter().filter_map(|t| t.strip_prefix("language_")).filter(|l| !l.is_empty() && l.len() <= 8).map(str::to_string).collect()
}

/// `#1a2B3c` → RGB.
pub fn colour(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// An ISO 8601 date-time (or a bare `YYYY-MM-DD`) as Unix seconds.
pub fn seconds(s: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    let s = s.trim();
    if let Ok(t) = time::OffsetDateTime::parse(s, &Rfc3339) {
        return Some(t.unix_timestamp());
    }
    let date = time::Date::parse(s.get(..10)?, time::macros::format_description!("[year]-[month]-[day]")).ok()?;
    Some(date.midnight().assume_utc().unix_timestamp())
}

/// An instance id's kind and region: `~hidden(usr)` is Friends+, `~friends(usr)` Friends, `~private(usr)` Invite (with
/// `~canRequestInvite`, Invite+), `~group(grp)~groupAccessType(public|plus|members)` the group kinds; none: Public.
pub fn access(instance: &str) -> (InstanceAccess, String) {
    let parts: Vec<&str> = instance.split('~').skip(1).collect();
    let arg = |name: &str| parts.iter().find_map(|p| p.strip_prefix(name).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')')));
    let region = arg("region").unwrap_or("us").to_string();
    let kind = if arg("hidden").is_some() {
        InstanceAccess::FriendsPlus
    } else if arg("friends").is_some() {
        InstanceAccess::Friends
    } else if arg("private").is_some() {
        if parts.contains(&"canRequestInvite") { InstanceAccess::InvitePlus } else { InstanceAccess::Invite }
    } else if arg("group").is_some() {
        match arg("groupAccessType") {
            Some("public") => InstanceAccess::GroupPublic,
            Some("plus") => InstanceAccess::GroupPlus,
            _ => InstanceAccess::Group,
        }
    } else {
        InstanceAccess::Public
    };
    (kind, region)
}

/// `wrld_…:instance` as a [`Place`]; anything else (`private`, `offline`, `traveling`, empty) is not one.
pub fn place(location: &str, world_name: &str) -> Option<Place> {
    let (world, instance) = location.split_once(':')?;
    if !crate::vrchat_log::is_world_id(world) || instance.is_empty() || instance.len() > 512 {
        return None;
    }
    let (access, region) = access(instance);
    Some(Place { world_id: world.into(), world_name: world_name.into(), world_picture: String::new(), instance_id: instance.into(), access, region })
}

pub fn whereabouts(location: &str) -> Whereabouts {
    match location {
        "private" => Whereabouts::Private,
        "traveling" | "traveling:traveling" => Whereabouts::Traveling,
        "offline" | "" => Whereabouts::Offline,
        other => place(other, "").map_or(Whereabouts::Unknown, Whereabouts::World),
    }
}

/// The decorations a user wears, as VRChat's inventory template ids (`iconFrame`, `nameplateEffect`): the controller
/// resolves them to pictures (`/inventory/template/{id}`, cached).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decorations {
    pub icon_frame: Option<String>,
    pub nameplate: Option<String>,
}

pub fn decorations(v: &Value) -> Decorations {
    let id = |k: &str| Some(text(v, k)).filter(|s| !s.is_empty() && s.len() <= 128);
    Decorations { icon_frame: id("iconFrame"), nameplate: id("nameplateEffect") }
}

/// The small picture: the chosen icon, else the profile picture override, else the avatar's.
fn icon(v: &Value) -> String {
    ["userIcon", "profilePicOverrideThumbnail", "profilePicOverride", "currentAvatarThumbnailImageUrl", "currentAvatarImageUrl"]
        .into_iter()
        .map(|k| picture(v, k))
        .find(|u| !u.is_empty())
        .unwrap_or_default()
}

/// The look of a user answer; decorations come empty (see [`Decorations`]).
pub fn look(v: &Value) -> Look {
    let tags = tags(v);
    let banner = picture(v, "bannerUrl");
    Look {
        banner_colour: if banner.is_empty() { colour(&text(v, "bannerColor")) } else { None },
        banner,
        nameplate: String::new(),
        icon_frame: String::new(),
        icon: icon(v),
        rank: rank(&tags),
        supporter: tags.contains(&"system_supporter"),
    }
}

/// A friend from a `LimitedUserFriend` / `User`. `online_list`: it came from the friends list asked with
/// `offline=false`, where someone only on the website has the location `offline` (VRChat has no other sign of it there).
pub fn friend(v: &Value, online_list: bool) -> Option<Friend> {
    let id = text(v, "id");
    if !crate::vrchat_accounts::is_user_id(&id) {
        return None;
    }
    let location = text(v, "location");
    let whereabouts = whereabouts(&location);
    let platform_text = text(v, "platform");
    let in_game = matches!(whereabouts, Whereabouts::World(_) | Whereabouts::Private | Whereabouts::Traveling);
    let state = text(v, "state");
    let presence = if in_game || state == "online" {
        Presence::InGame
    } else if state == "active" || (online_list && state != "offline") || platform_text == "web" && online_list {
        Presence::Website
    } else {
        Presence::Offline
    };
    let whereabouts = match presence {
        Presence::Website => Whereabouts::Website,
        Presence::InGame if !in_game => Whereabouts::Private,
        _ => whereabouts,
    };
    let platform = match platform(&platform_text) {
        Platform::Unknown => platform(&text(v, "last_platform")),
        known => known,
    };
    let last_seen = [seconds(&text(v, "last_activity")), seconds(&text(v, "last_login"))].into_iter().flatten().max();
    Some(Friend {
        name: Some(text(v, "displayName")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone()),
        id,
        look: look(v),
        status: if presence == Presence::Offline { Status::Offline } else { status(&text(v, "status")) },
        status_text: text(v, "statusDescription"),
        presence,
        whereabouts,
        platform: if presence == Presence::Website { Platform::Web } else { platform },
        last_seen,
        note: text(v, "note"),
        favourite_groups: Vec::new(),
        boop_ready_at: None,
        request_ready_at: None,
    })
}

/// The account's friend favourite groups from `/favorite/groups?type=friend`: `(name, shown name)` in VRChat's order.
pub fn favourite_groups(list: &Value) -> Vec<(String, String)> {
    let mut groups: Vec<(String, String)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| text(g, "type") == "friend")
        .filter_map(|g| {
            let name = text(g, "name");
            let shown = Some(text(g, "displayName")).filter(|s| !s.is_empty()).unwrap_or_else(|| name.clone());
            (!name.is_empty() && name.len() <= 64).then_some((name, shown))
        })
        .collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    groups.dedup_by(|a, b| a.0 == b.0);
    groups
}

/// Who is in which favourite group, from `/favorites?type=friend`: user id → group names (its `tags`).
pub fn favourites(list: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in list.as_array().into_iter().flatten().filter(|f| text(f, "type") == "friend") {
        let user = text(f, "favoriteId");
        if crate::vrchat_accounts::is_user_id(&user) {
            out.entry(user).or_default().extend(tags(f).into_iter().map(str::to_string));
        }
    }
    out
}

/// Rebuilds [`LauncherState::groups`] from the friends: the favourite groups (kept in their order, with their names),
/// then in game, website, offline. Each section's members by name; offline by last seen, newest first.
pub fn regroup(state: &mut LauncherState, favourite_groups: &[(String, String)]) {
    let mut by_name: Vec<&Friend> = state.friends.iter().collect();
    by_name.sort_by_key(|f| f.name.to_lowercase());
    let mut groups: Vec<FriendGroup> = favourite_groups
        .iter()
        .map(|(id, name)| FriendGroup {
            id: id.clone(),
            name: name.clone(),
            kind: GroupKind::Favourite,
            members: by_name.iter().filter(|f| f.favourite_groups.contains(id)).map(|f| f.id.clone()).collect(),
        })
        .collect();
    for (id, kind, presence) in [("in-game", GroupKind::InGame, Presence::InGame), ("website", GroupKind::Website, Presence::Website), ("offline", GroupKind::Offline, Presence::Offline)] {
        let mut members: Vec<&Friend> = by_name.iter().copied().filter(|f| f.presence == presence).collect();
        if presence == Presence::Offline {
            members.sort_by_key(|f| std::cmp::Reverse(f.last_seen));
        }
        groups.push(FriendGroup { id: id.into(), name: String::new(), kind, members: members.into_iter().map(|f| f.id.clone()).collect() });
    }
    state.groups = groups;
}

/// An invite or an invite request from a (v1) `Notification`.
pub fn invite(v: &Value) -> Option<Invite> {
    let kind = match text(v, "type").as_str() {
        "invite" => InviteKind::Invite,
        "requestInvite" => InviteKind::Request,
        _ => return None,
    };
    let details = decoded(v.get("details"));
    let id = text(v, "id");
    let from_id = text(v, "senderUserId");
    if id.is_empty() || id.len() > 128 || !crate::vrchat_accounts::is_user_id(&from_id) {
        return None;
    }
    let place = match kind {
        InviteKind::Invite => Some(place(&text(&details, "worldId"), &text(&details, "worldName"))?),
        InviteKind::Request => None,
    };
    let message = match kind {
        InviteKind::Invite => text(&details, "inviteMessage"),
        InviteKind::Request => text(&details, "requestMessage"),
    };
    Some(Invite {
        id,
        kind,
        from_id,
        from_name: text(v, "senderUsername"),
        from_icon: String::new(),
        place,
        message,
        received_at: seconds(&text(v, "created_at")).unwrap_or_default(),
    })
}

/// A boop from a `NotificationV2` (type `boop`), or from a v1 `Notification` of type `boop`.
pub fn boop(v: &Value) -> Option<Boop> {
    if text(v, "type") != "boop" {
        return None;
    }
    let id = text(v, "id");
    let from_id = text(v, "senderUserId");
    if id.is_empty() || id.len() > 128 || !crate::vrchat_accounts::is_user_id(&from_id) {
        return None;
    }
    let details = decoded(v.get("details"));
    let data = decoded(v.get("data"));
    let emoji = Some(text(&details, "emojiId")).filter(|e| !e.is_empty() && e.len() <= 128);
    let from_name = Some(text(&data, "boopingUserDisplayName")).filter(|n| !n.is_empty()).unwrap_or_else(|| text(v, "senderUsername"));
    let at = Some(text(v, "createdAt")).filter(|s| !s.is_empty()).unwrap_or_else(|| text(v, "created_at"));
    Some(Boop { id, from_id, from_name, from_icon: picture(v, "imageUrl"), emoji, received_at: seconds(&at).unwrap_or_default() })
}

/// The boop emojis offered: a few of VRChat's default emoji ids (`DefaultEmojiID` in the spec) with a Unicode stand-in.
pub fn boop_emojis() -> Vec<BoopEmoji> {
    [
        ("default_hand_wave", "👋"),
        ("default_heart", "❤️"),
        ("default_smile", "😊"),
        ("default_laugh", "😂"),
        ("default_in_love", "😍"),
        ("default_kiss", "😘"),
        ("default_confetti", "🎉"),
        ("default_fire", "🔥"),
        ("default_music_note", "🎵"),
        ("default_question", "❓"),
    ]
    .into_iter()
    .map(|(id, glyph)| BoopEmoji { id: id.into(), picture: String::new(), glyph: glyph.into() })
    .collect()
}

/// The profile modal's details: `profile` is `/profile/{id}` (`PublicProfile`), `user` is `/users/{id}` (`User`: note,
/// date joined, last seen), `mutuals` `/users/{id}/mutuals`. Any of them may be missing (`Value::Null`).
pub fn profile(base: Friend, profile: &Value, user: &Value, mutuals: Option<(u32, u32)>) -> Profile {
    let mut friend = base;
    let both = |k: &str| Some(text(profile, k)).filter(|s| !s.is_empty()).unwrap_or_else(|| text(user, k));
    let profile_tags = tags(profile);
    let languages = {
        let listed: Vec<String> = profile.get("languages").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if listed.is_empty() { self::languages(&if profile_tags.is_empty() { tags(user) } else { profile_tags }) } else { listed }
    };
    let badges = profile
        .get("badges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| !b.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .map(|b| Badge { id: text(b, "badgeId"), name: text(b, "badgeName"), description: text(b, "badgeDescription"), picture: picture(b, "badgeImageUrl") })
        .filter(|b| !b.id.is_empty())
        .take(64)
        .collect();
    let links = profile
        .get("bioLinks")
        .or_else(|| user.get("bioLinks"))
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).filter(|u| u.starts_with("https://") || u.starts_with("http://")).take(8).map(str::to_string).collect())
        .unwrap_or_default();
    let note = text(user, "note");
    if !note.is_empty() {
        friend.note = note;
    }
    if !profile.is_null() {
        let mut fresh = look(profile);
        if fresh.icon.is_empty() {
            fresh.icon = friend.look.icon.clone();
        }
        fresh.nameplate = std::mem::take(&mut friend.look.nameplate);
        fresh.icon_frame = std::mem::take(&mut friend.look.icon_frame);
        if fresh.rank == Rank::Visitor {
            fresh.rank = friend.look.rank;
        }
        friend.look = fresh;
    }
    let seen = [seconds(&text(user, "last_activity")), seconds(&text(user, "last_login"))].into_iter().flatten().max();
    friend.last_seen = friend.last_seen.max(seen);
    let large = ["profilePicOverride", "currentAvatarImageUrl"].into_iter().map(|k| Some(picture(profile, k)).filter(|u| !u.is_empty()).unwrap_or_else(|| picture(user, k))).find(|u| !u.is_empty()).unwrap_or_default();
    Profile {
        pronouns: both("pronouns"),
        bio: both("bio"),
        links,
        languages,
        badges,
        picture: if large.is_empty() { friend.look.icon.clone() } else { large },
        date_joined: both("date_joined").chars().take(10).collect(),
        friends_since: None,
        mutual_friends: mutuals.map(|m| m.0),
        mutual_groups: mutuals.map(|m| m.1),
        loading: false,
        friend,
    }
}

/// What applying a pipeline message did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The friends list or sections changed (redraw; [`regroup`] was already run).
    pub changed: bool,
    /// Decorations to resolve to pictures, by user id.
    pub decorations: Vec<(String, Decorations)>,
    /// A new invite, request or boop arrived (the page may call attention to it).
    pub arrived: bool,
    /// The message said something about the account itself (`user-update`, `user-location`).
    pub me: bool,
}

/// Applies one pipeline message (`{"type": …, "content": …}`, `content` JSON-encoded) to the state. Unknown types change
/// nothing. `now` dates an offline friend's last seen.
pub fn apply(state: &mut LauncherState, favourite_groups: &[(String, String)], message: &str, now: i64) -> Applied {
    let mut out = Applied::default();
    let Ok(message) = serde_json::from_str::<Value>(message) else { return out };
    let kind = text(&message, "type");
    let content = decoded(message.get("content"));
    let user_id = text(&content, "userId");
    let user = content.get("user").cloned().unwrap_or(Value::Null);
    let keep = |old: Option<&Friend>, mut new: Friend| {
        if let Some(old) = old {
            new.favourite_groups = old.favourite_groups.clone();
            new.boop_ready_at = old.boop_ready_at;
            new.request_ready_at = old.request_ready_at;
            if new.note.is_empty() {
                new.note = old.note.clone();
            }
            new.look.nameplate = old.look.nameplate.clone();
            new.look.icon_frame = old.look.icon_frame.clone();
            new.last_seen = new.last_seen.max(old.last_seen);
        }
        new
    };
    let index = state.friends.iter().position(|f| f.id == user_id);
    match kind.as_str() {
        "friend-online" | "friend-location" | "friend-active" | "friend-update" | "friend-add" => {
            let mut v = user.clone();
            if !v.is_object() {
                v = serde_json::json!({});
            }
            v["id"] = Value::String(user_id.clone());
            let old = index.map(|i| state.friends[i].clone());
            match kind.as_str() {
                "friend-online" | "friend-location" => {
                    let location = text(&content, "location");
                    let location = if location == "traveling" { "traveling".to_string() } else { location };
                    v["location"] = Value::String(if location.is_empty() { "private".into() } else { location });
                    v["state"] = Value::String("online".into());
                }
                "friend-active" => {
                    v["location"] = Value::String("offline".into());
                    v["state"] = Value::String("active".into());
                }
                _ => {
                    // An update keeps where they are.
                    if let Some(old) = &old {
                        v["state"] = Value::String(match old.presence { Presence::InGame => "online", Presence::Website => "active", Presence::Offline => "offline" }.into());
                        let location = match &old.whereabouts {
                            Whereabouts::World(p) => format!("{}:{}", p.world_id, p.instance_id),
                            Whereabouts::Private => "private".into(),
                            Whereabouts::Traveling => "traveling".into(),
                            _ => "offline".into(),
                        };
                        v["location"] = Value::String(location);
                    }
                }
            }
            if let Some(p) = content.get("platform").and_then(Value::as_str) {
                v["platform"] = Value::String(p.into());
            }
            if user.get("displayName").is_none() {
                if let Some(old) = &old {
                    v["displayName"] = Value::String(old.name.clone());
                }
            }
            let Some(mut new) = friend(&v, false) else { return out };
            if user.is_null() {
                if let Some(old) = &old {
                    new.look = old.look.clone();
                    new.status = if new.presence == Presence::Offline { Status::Offline } else { old.status };
                    new.status_text = old.status_text.clone();
                }
            }
            // The world's name, kept when the place is the same, else from the message's `world` when it has one.
            if let (Whereabouts::World(p), Some(old)) = (&mut new.whereabouts, &old) {
                if let Whereabouts::World(was) = &old.whereabouts {
                    if was.world_id == p.world_id {
                        p.world_name = was.world_name.clone();
                        p.world_picture = was.world_picture.clone();
                    }
                }
            }
            if let Whereabouts::World(p) = &mut new.whereabouts {
                let world = decoded(content.get("world"));
                if text(&world, "id") == p.world_id {
                    p.world_name = text(&world, "name");
                    p.world_picture = picture(&world, "thumbnailImageUrl");
                }
            }
            let new = keep(old.as_ref(), new);
            if !user.is_null() {
                out.decorations.push((new.id.clone(), decorations(&user)));
            }
            if old.as_ref() != Some(&new) {
                match index {
                    Some(i) => state.friends[i] = new,
                    None if kind != "friend-update" => state.friends.push(new),
                    None => {}
                }
                out.changed = true;
            }
        }
        "friend-offline" => {
            if let Some(i) = index {
                let f = &mut state.friends[i];
                f.presence = Presence::Offline;
                f.whereabouts = Whereabouts::Offline;
                f.status = Status::Offline;
                f.last_seen = Some(now);
                out.changed = true;
            }
        }
        "friend-delete" => {
            if let Some(i) = index {
                state.friends.remove(i);
                out.changed = true;
            }
        }
        "notification" | "notification-v2" => {
            if let Some(invite) = invite(&content) {
                if !state.invites.iter().any(|i| i.id == invite.id) {
                    state.invites.insert(0, with_icon(state, invite));
                    out.arrived = true;
                }
            } else if let Some(mut boop) = boop(&content) {
                if !state.boops.iter().any(|b| b.id == boop.id) {
                    if boop.from_icon.is_empty() {
                        boop.from_icon = state.friend(&boop.from_id).map(|f| f.look.icon.clone()).unwrap_or_default();
                    }
                    if boop.received_at == 0 {
                        boop.received_at = now;
                    }
                    state.boops.insert(0, boop);
                    out.arrived = true;
                }
            }
        }
        "notification-v2-delete" => {
            let ids: Vec<String> = content.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            state.invites.retain(|i| !ids.contains(&i.id));
            state.boops.retain(|b| !ids.contains(&b.id));
        }
        "hide-notification" | "clear-notification" => {
            // `hide-notification`'s content is the bare id; `clear-notification` clears all.
            let id = message.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
            if kind == "clear-notification" {
                state.invites.clear();
            } else {
                state.invites.retain(|i| i.id != id);
                state.boops.retain(|b| b.id != id);
            }
        }
        "user-update" | "user-location" => {
            out.me = true;
            if let Some(me) = &mut state.me {
                if !user.is_null() {
                    let mut v = user.clone();
                    v["id"] = Value::String(me.id.clone());
                    v["state"] = Value::String("online".into());
                    let location = text(&content, "location");
                    v["location"] = Value::String(if location.is_empty() { "private".into() } else { location });
                    if let Some(new) = friend(&v, false) {
                        if user.get("displayName").is_some() {
                            me.name = new.name;
                            me.status = new.status;
                            me.status_text = new.status_text;
                            me.look.icon = if new.look.icon.is_empty() { me.look.icon.clone() } else { new.look.icon };
                        }
                        if kind == "user-location" {
                            me.whereabouts = new.whereabouts;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if out.changed {
        regroup(state, favourite_groups);
    }
    out
}

fn with_icon(state: &LauncherState, mut invite: Invite) -> Invite {
    if let Some(f) = state.friend(&invite.from_id) {
        invite.from_icon = f.look.icon.clone();
        if invite.from_name.is_empty() {
            invite.from_name = f.name.clone();
        }
    }
    invite
}

/// The per-person cool-down of boops and invite requests (our own, on top of VRChat's limits): one boop per person per
/// [`Cooldowns::BOOP`], one invite request per [`Cooldowns::REQUEST`].
#[derive(Clone, Debug, Default)]
pub struct Cooldowns {
    boops: BTreeMap<String, i64>,
    requests: BTreeMap<String, i64>,
}

impl Cooldowns {
    pub const BOOP: i64 = 60;
    pub const REQUEST: i64 = 300;

    /// `Ok(())` and the time is taken, or `Err(until)` when it is too soon.
    pub fn take(&mut self, boop: bool, to: &str, now: i64) -> Result<(), i64> {
        let (map, wait) = if boop { (&mut self.boops, Self::BOOP) } else { (&mut self.requests, Self::REQUEST) };
        if let Some(&until) = map.get(to).filter(|&&u| u > now) {
            return Err(until);
        }
        map.retain(|_, u| *u > now);
        map.insert(to.to_string(), now + wait);
        Ok(())
    }

    /// Gives a time back (the call failed before VRChat got it).
    pub fn release(&mut self, boop: bool, to: &str) {
        if boop { self.boops.remove(to) } else { self.requests.remove(to) };
    }

    /// Writes the times still running into the friends.
    pub fn mark(&self, friends: &mut [Friend], now: i64) {
        for f in friends {
            f.boop_ready_at = self.boops.get(&f.id).copied().filter(|&u| u > now);
            f.request_ready_at = self.requests.get(&f.id).copied().filter(|&u| u > now);
        }
    }
}

#[cfg(test)]
#[path = "launcher_feed_tests.rs"]
mod tests;
