//! N4N0 as a VRChat launcher: the shared shapes between the data side (friends, invites, boops, launching) and the
//! Home page that draws them (REQUEST10). This file is the contract of the `core` and `ui` streams: the page reads a
//! [`LauncherState`] and sends [`Action`]s; [`sample`] is a set of made-up people to draw without any account.
//!
//! Times are Unix seconds (UTC). Picture fields are URLs as VRChat gives them (empty when there is none); the
//! controller turns them into cached textures. Nothing here talks to the network.
use serde::{Deserialize, Serialize};

pub use crate::vrchat_api::InstanceAccess;

/// Where the friends come from, which decides what is possible ([`Possible`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Source {
    /// Nothing to show yet: no VRChat sign-in and no VRCX / VRCNext data found.
    #[default]
    Nothing,
    /// The VRChat account signed in in N4N0 (Settings > VRChat account), kept live through VRChat's pipeline.
    SignedIn { user_id: String, name: String },
    /// A copy of VRCX's or VRCNext's own database: names, pictures, last seen, favourite groups, notes; no live status.
    Database { app: DatabaseApp, read_at: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseApp { Vrcx, VrcNext }

/// What the page can offer right now. Everything that reaches VRChat needs [`Source::SignedIn`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Possible {
    /// Status, status text and location are live (pipeline) rather than a last-known copy.
    pub live_status: bool,
    pub invites: bool,
    pub request_invite: bool,
    pub boop: bool,
    /// VRChat is installed and can be started (or is running and can be sent somewhere).
    pub play: bool,
}

/// VRChat's own status, the coloured dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// "join me", blue.
    JoinMe,
    /// "online" / "active", green.
    Active,
    /// "ask me", orange.
    AskMe,
    /// "busy" / "do not disturb", red.
    Busy,
    #[default]
    Offline,
}

/// Which section of the list a friend falls in, besides their favourite groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// In the game.
    InGame,
    /// Not in the game, but active on the website or the companion app.
    Website,
    #[default]
    Offline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    /// Windows (`standalonewindows`).
    Pc,
    /// Quest / Android (`android`).
    Android,
    Ios,
    /// The website or the companion app.
    Web,
    #[default]
    Unknown,
}

/// VRChat's trust rank, which colours the name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rank {
    #[default]
    Visitor,
    NewUser,
    User,
    KnownUser,
    TrustedUser,
}

/// A world instance someone is in or is invited to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// `wrld_…`.
    pub world_id: String,
    /// Empty until the world's name is known.
    pub world_name: String,
    /// World thumbnail, empty until known.
    pub world_picture: String,
    /// The full instance id after the `:` (`12345~friends(usr_…)~region(eu)`), exactly as VRChat gave it.
    pub instance_id: String,
    pub access: InstanceAccess,
    /// `us`, `use`, `eu`, `jp`; empty when not said.
    pub region: String,
}

/// Where a friend is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Whereabouts {
    World(Place),
    /// In the game, in a private (invite / hidden) instance.
    Private,
    /// Between two worlds.
    Traveling,
    /// On the website or the companion app.
    Website,
    #[default]
    Offline,
    /// The source does not say (a database copy).
    Unknown,
}

/// How a person's profile looks on their row and in the modal ("profile customizations").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Look {
    /// Picture behind the row and at the top of the modal; empty: none.
    pub banner: String,
    /// The small round picture: VRChat's chosen icon, else the profile picture override, else the avatar's picture.
    pub icon: String,
    pub rank: Rank,
    /// VRC+ supporter (VRChat shows a badge).
    pub supporter: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Friend {
    /// `usr_…`.
    pub id: String,
    pub name: String,
    pub look: Look,
    pub status: Status,
    /// Their own status line ("Ask Me - orb orb orb"); may be empty.
    pub status_text: String,
    pub presence: Presence,
    pub whereabouts: Whereabouts,
    pub platform: Platform,
    /// Last time seen online, when known.
    pub last_seen: Option<i64>,
    /// The account's own private note about them.
    pub note: String,
    /// Ids of the favourite groups ([`FriendGroup::id`]) they are in.
    pub favourite_groups: Vec<String>,
    /// When a boop / an invite request to them is allowed again (our own per-person cool-down); `None`: now.
    pub boop_ready_at: Option<i64>,
    pub request_ready_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupKind {
    /// A favourite friend group ("friend playlist"), with its own name.
    Favourite,
    InGame,
    Website,
    Offline,
}

/// One section of the list, in the order to draw: the account's favourite groups first (in VRChat's order), then in
/// game, on the website, offline. A favourite friend appears in their favourite group and in their presence section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FriendGroup {
    /// VRChat's group name (`group_0`…) for favourites; `in-game` / `website` / `offline` for the others.
    pub id: String,
    /// Shown name ("noice bois"); empty for the presence sections (the page names them in its language).
    pub name: String,
    pub kind: GroupKind,
    /// Friend ids in the order to draw.
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub id: String,
    pub name: String,
    pub description: String,
    pub picture: String,
}

/// Everything the profile modal shows. Fields the source does not give stay empty / `None`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub friend: Friend,
    pub pronouns: String,
    pub bio: String,
    pub links: Vec<String>,
    /// ISO 639-3 codes from VRChat's `language_` tags (`eng`, `jpn`).
    pub languages: Vec<String>,
    pub badges: Vec<Badge>,
    /// Large profile picture (override or avatar picture).
    pub picture: String,
    /// `YYYY-MM-DD`.
    pub date_joined: String,
    /// Since when they are friends with the account, when known.
    pub friends_since: Option<i64>,
    pub mutual_friends: Option<u32>,
    pub mutual_groups: Option<u32>,
    /// The details are still being read; the page shows `friend` meanwhile.
    pub loading: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InviteKind {
    /// They invite the account to their instance: "accept and play".
    Invite,
    /// They ask the account for an invite.
    Request,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    /// VRChat's notification id (`not_…`).
    pub id: String,
    pub kind: InviteKind,
    pub from_id: String,
    pub from_name: String,
    pub from_icon: String,
    /// The instance invited to ([`InviteKind::Invite`]); `None` for a request.
    pub place: Option<Place>,
    pub message: String,
    pub received_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Boop {
    pub id: String,
    pub from_id: String,
    pub from_name: String,
    pub from_icon: String,
    /// VRChat's emoji id when the boop carried one, else `None` (a plain boop).
    pub emoji: Option<String>,
    pub received_at: i64,
}

/// An emoji a boop can carry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoopEmoji {
    /// What to send (VRChat's id).
    pub id: String,
    /// What to show: a picture URL, or empty when `glyph` is used.
    pub picture: String,
    /// A Unicode stand-in when there is no picture.
    pub glyph: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchMode {
    #[default]
    Vr,
    Desktop,
}

/// What the game is doing now, for the Play buttons.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Game {
    #[default]
    NotRunning,
    /// Started by us a moment ago and not yet in a world.
    Starting(LaunchMode),
    /// Running (whoever started it); a Play then sends it to the destination instead of starting a second one.
    Running(LaunchMode),
    /// VRChat was not found on this computer.
    NotInstalled,
}

/// What the page sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Start VRChat (or, when it runs, go to `destination`).
    Play { mode: LaunchMode, destination: Option<Place> },
    /// Accept an invite and play it in that mode (or send the running game there).
    AcceptInvite { invite: String, mode: LaunchMode },
    /// Hide an invite / request / boop here (and mark it seen at VRChat when signed in).
    Dismiss { id: String },
    RequestInvite { to: String },
    /// Boop; `emoji` from [`LauncherState::boop_emojis`], `None` for a plain boop.
    Boop { to: String, emoji: Option<String> },
    /// Open the profile modal; the details arrive in [`LauncherState::profile`].
    OpenProfile { id: String },
    CloseProfile,
    /// Read everything again now (the pipeline keeps it live otherwise).
    Refresh,
}

/// The outcome of the last action, shown briefly by the page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Notice {
    Sent { to: String, action: SentKind },
    /// Already done too recently for this person; allowed again at that time.
    CoolingDown { to: String, until: i64 },
    /// A text in the page's language, ready to show.
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentKind { Boop, RequestInvite, Launch, Join }

/// What the Home page reads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LauncherState {
    pub source: Source,
    pub possible: Possible,
    /// The account itself (top of the list), when known.
    pub me: Option<Friend>,
    /// Every friend, by no particular order; [`FriendGroup::members`] give the order.
    pub friends: Vec<Friend>,
    pub groups: Vec<FriendGroup>,
    /// Newest first.
    pub invites: Vec<Invite>,
    /// Newest first.
    pub boops: Vec<Boop>,
    pub boop_emojis: Vec<BoopEmoji>,
    pub profile: Option<Profile>,
    pub game: Game,
    pub notice: Option<Notice>,
    /// Reading the friends for the first time.
    pub loading: bool,
}

impl LauncherState {
    pub fn friend(&self, id: &str) -> Option<&Friend> {
        self.friends.iter().find(|friend| friend.id == id)
    }
}

/// Made-up people for drawing the page with no account (and for pictures in release notes). Nobody real.
pub fn sample() -> LauncherState {
    const NOW: i64 = 1_795_000_000;
    let place = |world: &str, name: &str, instance: &str, access, region: &str| Place {
        world_id: world.into(),
        world_name: name.into(),
        world_picture: String::new(),
        instance_id: instance.into(),
        access,
        region: region.into(),
    };
    let friend = |n: u32, name: &str, status, text: &str, presence, whereabouts, platform, rank, groups: &[&str]| Friend {
        id: format!("usr_00000000-0000-4000-8000-{n:012}"),
        name: name.into(),
        look: Look { banner: String::new(), icon: String::new(), rank, supporter: n % 3 == 0 },
        status,
        status_text: text.into(),
        presence,
        whereabouts,
        platform,
        last_seen: Some(NOW - i64::from(n) * 3_600),
        note: if n == 2 { "Met at the sample meetup".into() } else { String::new() },
        favourite_groups: groups.iter().map(|g| (*g).into()).collect(),
        boop_ready_at: if n == 4 { Some(NOW + 40) } else { None },
        request_ready_at: None,
    };
    use Platform as P;
    use Presence as Pr;
    use Rank as R;
    use Status as S;
    let lounge = place("wrld_00000000-0000-4000-8000-000000000001", "Sample Lounge", "12345~region(eu)", InstanceAccess::Public, "eu");
    let cafe = place(
        "wrld_00000000-0000-4000-8000-000000000002",
        "Quiet Café",
        "777~friends(usr_00000000-0000-4000-8000-000000000003)~region(jp)",
        InstanceAccess::Friends,
        "jp",
    );
    let friends = vec![
        friend(1, "Nova Sample", S::JoinMe, "come hang", Pr::InGame, Whereabouts::World(lounge.clone()), P::Pc, R::TrustedUser, &["group_0"]),
        friend(2, "Pixel Fox", S::AskMe, "Ask me", Pr::InGame, Whereabouts::Private, P::Android, R::KnownUser, &["group_0", "group_1"]),
        friend(3, "Mochi Byte", S::Active, "", Pr::InGame, Whereabouts::World(cafe.clone()), P::Pc, R::TrustedUser, &["group_1"]),
        friend(4, "Orbit Tester", S::Busy, "working in Unity", Pr::InGame, Whereabouts::Traveling, P::Pc, R::User, &[]),
        friend(5, "Lumen", S::Active, "on my phone", Pr::Website, Whereabouts::Website, P::Web, R::NewUser, &[]),
        friend(6, "Quiet Kite", S::Offline, "zzz", Pr::Offline, Whereabouts::Offline, P::Unknown, R::KnownUser, &["group_1"]),
        friend(7, "Sample Seven", S::Offline, "", Pr::Offline, Whereabouts::Offline, P::Unknown, R::Visitor, &[]),
    ];
    let in_section = |presence: Presence| friends.iter().filter(|f| f.presence == presence).map(|f| f.id.clone()).collect();
    let in_group = |group: &str| {
        friends.iter().filter(|f| f.favourite_groups.iter().any(|g| g == group)).map(|f| f.id.clone()).collect()
    };
    let groups = vec![
        FriendGroup { id: "group_0".into(), name: "best people".into(), kind: GroupKind::Favourite, members: in_group("group_0") },
        FriendGroup { id: "group_1".into(), name: "café crew".into(), kind: GroupKind::Favourite, members: in_group("group_1") },
        FriendGroup { id: "in-game".into(), name: String::new(), kind: GroupKind::InGame, members: in_section(Pr::InGame) },
        FriendGroup { id: "website".into(), name: String::new(), kind: GroupKind::Website, members: in_section(Pr::Website) },
        FriendGroup { id: "offline".into(), name: String::new(), kind: GroupKind::Offline, members: in_section(Pr::Offline) },
    ];
    let me = friend(0, "Sample Me", S::AskMe, "Ask me - sample", Pr::InGame, Whereabouts::Private, P::Pc, R::TrustedUser, &[]);
    let invites = vec![
        Invite {
            id: "not_00000000-0000-4000-8000-000000000001".into(),
            kind: InviteKind::Invite,
            from_id: friends[2].id.clone(),
            from_name: friends[2].name.clone(),
            from_icon: String::new(),
            place: Some(cafe),
            message: "come to the café!".into(),
            received_at: NOW - 30,
        },
        Invite {
            id: "not_00000000-0000-4000-8000-000000000002".into(),
            kind: InviteKind::Request,
            from_id: friends[1].id.clone(),
            from_name: friends[1].name.clone(),
            from_icon: String::new(),
            place: None,
            message: String::new(),
            received_at: NOW - 300,
        },
    ];
    let boops = vec![Boop {
        id: "not_00000000-0000-4000-8000-000000000003".into(),
        from_id: friends[0].id.clone(),
        from_name: friends[0].name.clone(),
        from_icon: String::new(),
        emoji: None,
        received_at: NOW - 12,
    }];
    let boop_emojis = ["👋", "💖", "😺", "🎉"]
        .iter()
        .enumerate()
        .map(|(i, glyph)| BoopEmoji { id: format!("sample_{i}"), picture: String::new(), glyph: (*glyph).into() })
        .collect();
    let profile = Profile {
        friend: friends[0].clone(),
        pronouns: "they/them".into(),
        bio: "Made-up person for drawing the page.\nLikes quiet worlds.".into(),
        links: vec!["https://example.com/nova".into()],
        languages: vec!["eng".into(), "fra".into()],
        badges: vec![Badge { id: "bdg_sample".into(), name: "Sample badge".into(), description: "A made-up badge".into(), picture: String::new() }],
        picture: String::new(),
        date_joined: "2021-04-01".into(),
        friends_since: Some(NOW - 400 * 86_400),
        mutual_friends: Some(12),
        mutual_groups: Some(2),
        loading: false,
    };
    LauncherState {
        source: Source::SignedIn { user_id: me.id.clone(), name: me.name.clone() },
        possible: Possible { live_status: true, invites: true, request_invite: true, boop: true, play: true },
        me: Some(me),
        friends,
        groups,
        invites,
        boops,
        boop_emojis,
        profile: Some(profile),
        game: Game::NotRunning,
        notice: None,
        loading: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_is_consistent() {
        let state = sample();
        for group in &state.groups {
            for id in &group.members {
                assert!(state.friend(id).is_some(), "{id} in {} is a friend", group.id);
            }
        }
        let sections: usize = state.groups.iter().filter(|g| g.kind != GroupKind::Favourite).map(|g| g.members.len()).sum();
        assert_eq!(sections, state.friends.len(), "every friend is in exactly one presence section");
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<LauncherState>(&json).unwrap(), state);
    }
}
