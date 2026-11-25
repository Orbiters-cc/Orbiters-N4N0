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
    /// VRChat's banner colour (`bannerType` `color`), RGB; drawn when there is no banner picture.
    pub banner_colour: Option<[u8; 3]>,
    /// VRChat's nameplate decoration (an inventory item's picture, drawn behind the row); empty: none.
    pub nameplate: String,
    /// VRChat's icon frame (an inventory item's picture, drawn around the icon); empty: none.
    pub icon_frame: String,
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
    /// A Mac without what runs VRChat there ([`crate::obelisk`]).
    NoEngine,
}

/// Setting VRChat up on a Mac ([`crate::obelisk`]), from the press of "Install & play VRChat" to the game.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EngineSetup {
    /// What runs VRChat on a Mac is coming, this much of it.
    Downloading { percent: u8 },
    /// It is checked (whose signature it carries) and put in place.
    Installing,
    /// The person signs in to Steam: this address, shown as a QR code, is read with the Steam app on their phone.
    SignIn { qr_url: String },
    /// Steam is asked what to download, or the files there are read.
    Checking,
    /// The engine downloads (`game` false) or VRChat does (`game` true), this much when it is known.
    Game { game: bool, percent: Option<u8> },
    Failed(String),
}

/// One of VRChat's local profiles (`--profile=N`, REQUEST15 7a): each keeps its own signed-in account, so several
/// VRChats run side by side only on different profiles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameProfile {
    /// VRChat's `--profile` number (0: the default, a start without `--profile`).
    pub profile: u32,
    /// The account last seen signed in on it (its VRChat log said so), when known.
    pub account: Option<String>,
    /// A VRChat runs on it now, in this mode.
    pub running: Option<LaunchMode>,
    /// Started from here a moment ago and not yet seen running.
    pub starting: Option<LaunchMode>,
    /// The VRChat on it is the assistant's (the VRChat bridge's), not the person's.
    pub assistant: bool,
}

/// What a status can be set to from here: VRChat's four ("offline" is not one a person picks).
pub const SETTABLE_STATUSES: [Status; 4] = [Status::JoinMe, Status::Active, Status::AskMe, Status::Busy];
/// VRChat's limits on the account's own words (its website and VRCX hold the same): status text, bio.
pub const STATUS_TEXT_MAX: usize = 32;
pub const BIO_MAX: usize = 512;

/// The account's banner, as VRChat's profile has it (`bannerType`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum BannerChoice {
    /// A colour (`bannerColor`, RGB).
    #[default]
    Colour,
    /// The avatar's own picture.
    Avatar,
    /// A picture of the account's VRChat gallery (`bannerCustomUrl`).
    Custom,
}

/// A picture of the account's own (an icon or a gallery picture uploaded in VRChat).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OwnPicture {
    /// `file_…`.
    pub id: String,
    /// The file's latest version, as VRChat names it for `userIcon` / `bannerCustomUrl`.
    pub url: String,
}

/// Which list of the account's own pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PictureKind {
    /// VRChat's icons (`/files?tag=icon`): the profile picture.
    Icons,
    /// VRChat's gallery (`/files?tag=gallery`): the banner.
    Gallery,
}

/// What of the account's own profile is being changed (one at a time; REQUEST15 7b).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfilePart { Status, StatusText, Bio, Picture, Banner }

/// A change of the account's own profile, sent only on the person's own press.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProfileEdit {
    Status(Status),
    StatusText(String),
    Bio(String),
    /// A picture of [`PictureKind::Icons`] by its URL; empty: the avatar's picture again.
    Picture(String),
    Banner { choice: BannerChoice, colour: Option<[u8; 3]>, custom: String },
}

impl ProfileEdit {
    pub fn part(&self) -> ProfilePart {
        match self {
            ProfileEdit::Status(_) => ProfilePart::Status,
            ProfileEdit::StatusText(_) => ProfilePart::StatusText,
            ProfileEdit::Bio(_) => ProfilePart::Bio,
            ProfileEdit::Picture(_) => ProfilePart::Picture,
            ProfileEdit::Banner { .. } => ProfilePart::Banner,
        }
    }
}

/// How the last change of a part went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Saving {
    Saving(ProfilePart),
    Saved(ProfilePart),
    /// Not changed at VRChat: why, in the page's language.
    Failed(ProfilePart, String),
}

/// The account's own profile as Home's card edits it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MyProfile {
    /// Read from VRChat (`launcher_mine`): until then the card holds a placeholder (made by a list or a change) whose
    /// empty bio is not the account's, and no bio can be saved from it.
    #[serde(default)]
    pub loaded: bool,
    pub bio: String,
    pub banner: BannerChoice,
    pub banner_colour: Option<[u8; 3]>,
    /// The gallery picture of [`BannerChoice::Custom`].
    pub banner_custom: String,
    /// The chosen icon (`userIcon`); empty: the avatar's picture shows.
    pub icon: String,
    /// The avatar's own picture (what shows without an icon, and the Avatar banner).
    pub avatar_picture: String,
    /// VRC+: own icons and gallery pictures are VRC+'s.
    pub supporter: bool,
    /// The account's icons and gallery pictures, once asked for (`None`: not read yet).
    pub icons: Option<Vec<OwnPicture>>,
    pub gallery: Option<Vec<OwnPicture>>,
    /// Reading one of the lists now.
    pub listing: Option<PictureKind>,
    /// A list that could not be read, and why (said in its chooser, with its own Try again).
    #[serde(default)]
    pub list_failed: Option<(PictureKind, String)>,
    pub saving: Option<Saving>,
    /// Another change is allowed from then on (Unix seconds): VRChat refuses changes in quick succession.
    pub next_change_at: Option<i64>,
    /// How many changes were answered (saved or not) so far: the page knows when its own press is answered.
    pub answered: u64,
}

/// What the page sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Start VRChat (or, when it runs, go to `destination`).
    Play { mode: LaunchMode, destination: Option<Place> },
    /// Start a VRChat on this profile, beside any running on others; never a second one on the same profile.
    PlayOn { mode: LaunchMode, profile: u32 },
    /// [`Action::PlayOn`] right into `destination` (Home's Where, REQUEST20 2): the instance's link goes to VRChat as it
    /// starts on `profile`. Never sent to a VRChat already running on that profile.
    PlayAt { mode: LaunchMode, profile: u32, destination: Place },
    /// Change the account's own profile (status, its text, bio, picture, banner) at VRChat.
    EditProfile(ProfileEdit),
    /// Read the account's own icons or gallery pictures, to choose among.
    ListPictures(PictureKind),
    /// Read the account's own profile (bio, icon, banner) for Home's card.
    ReadMine,
    /// Send the VRChat that runs or starts here to `destination`; none is ever started for it.
    JoinRunning { mode: LaunchMode, destination: Place },
    /// Accept an invite and play it in that mode (or send the running game there).
    AcceptInvite { invite: String, mode: LaunchMode },
    /// Accept an invite into the VRChat that runs or starts here; none is ever started for it.
    AcceptInviteRunning { invite: String, mode: LaunchMode },
    /// [`Action::AcceptInvite`] starting VRChat on `profile` (Home's Where, REQUEST20 2): the same one accept per invite,
    /// the same card going and coming back, the same hide at VRChat once there.
    AcceptInviteOn { invite: String, mode: LaunchMode, profile: u32 },
    /// Hide an invite / request / boop here (and mark it seen at VRChat when signed in).
    Dismiss { id: String },
    RequestInvite { to: String },
    /// Boop; `emoji` from [`LauncherState::boop_emojis`], `None` for a plain boop.
    Boop { to: String, emoji: Option<String> },
    /// Try again to go to the place that was not delivered ([`Joining`]).
    JoinAgain,
    /// Give that place up.
    ForgetJoining,
    /// Take a profile that does not run off Home's choice: what is remembered of it here goes (VRChat's own sign-in on it stays).
    ForgetProfile { profile: u32 },
    /// On a Mac where the game is started by hand in another app: open that app.
    OpenEngine,
    /// On a Mac where VRChat is not set up: get what runs it, sign in to Steam, download VRChat, then play it on `profile`.
    InstallEngine { profile: u32 },
    /// On a Mac: move the engine's damaged start record `file` ([`StartTrouble`]) out of its records, kept aside.
    SetAsideRecord { file: String },
    /// On a Mac: the VRChat of Game session `session`, which Home showed as having lost its picture for good
    /// ([`LauncherState::picture_lost`]): quit that very session and start it again as it was started, through the one
    /// start (checked again first; nothing for a healthy game, nor for another session's).
    RestartLostGame { session: String },
    /// On a Mac: install what VRChat's engine is made of again ([`LauncherState::engine_trouble`]).
    RepairEngine,
    /// Open the profile modal; the details arrive in [`LauncherState::profile`].
    OpenProfile { id: String },
    CloseProfile,
    /// Read everything again now (the pipeline keeps it live otherwise).
    Refresh,
    /// `action`, pressed on the list of the VRChat account `account` (the user id of [`Source::SignedIn`] on show at the
    /// press). Whatever reaches a person or VRChat (a boop, a request, an accepted invite, a hide, an invite to
    /// oneself) goes out only with that same account's session, checked again right before it is sent.
    As { account: String, action: Box<Action> },
}

/// The outcome of the last action, shown briefly by the page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Notice {
    Sent { to: String, action: SentKind },
    /// Already done too recently for this person; allowed again at that time.
    CoolingDown { to: String, until: i64 },
    /// A text in the page's language, ready to show.
    Error(String),
    /// A boop or an invite request to `to` that did not go (or may not have): the page takes back its "Sent" and shows
    /// `text`, in the page's language.
    Failed { to: String, action: SentKind, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentKind { Boop, RequestInvite, Launch, Join }

/// Whom what VRChat said of an account was verified for: the Orbiters identity's epoch, the member (a digest of the
/// environment and their id: never anything else) and the VRChat account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VerifiedFor {
    pub epoch: u64,
    pub member: String,
    #[serde(default)]
    pub user: Option<String>,
}

/// What the Home page reads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LauncherState {
    pub source: Source,
    /// Whom a signed-in account's list was verified for; none for this computer's own copy (verify3-r21 B6).
    #[serde(default)]
    pub verified_for: Option<VerifiedFor>,
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
    /// The list is the one last on show, kept on this computer ([`crate::vrchat_store`]), and VRChat has not said anything
    /// since: when it was last live. `None` once read.
    #[serde(default)]
    pub kept: Option<i64>,
    /// VRChat's profiles on this computer: the ones running, starting or used before, by number.
    #[serde(default)]
    pub profiles: Vec<GameProfile>,
    /// VRChats running here whose profile is not known yet (neither their log nor their command line said it): Play
    /// waits for them, and Home says so.
    #[serde(default)]
    pub unresolved: u32,
    /// The signed-in account's own profile, for Home's card (`None` until read, and with no sign-in).
    #[serde(default)]
    pub mine: Option<MyProfile>,
    /// On a Mac: how getting Obelisk goes, from the press of its button until VRChat is in it.
    #[serde(default)]
    pub engine_setup: Option<EngineSetup>,
    /// On a Mac: the VRChat Home started still runs and its graphics went for good (the engine's graphics server
    /// dropped that very session, and no frame came since): Home offers to start it again. Never for a game that is only
    /// covered, behind the lock screen or slow.
    /// That VRChat's Game session: what the restart is of.
    #[serde(default)]
    pub picture_lost: Option<String>,
    /// On a Mac: the files of the engine no game starts on, and what ends it.
    #[serde(default)]
    pub engine_trouble: Option<EngineTrouble>,
    /// On a Mac: a record of the engine's that cannot be read, which Home names (and, once it is known to be history,
    /// offers to set aside).
    #[serde(default)]
    pub start_trouble: Option<StartTrouble>,
    /// This computer uses several VRChat profiles ([`crate::vrchat_launch::several`]): a private place goes to a VRChat in two
    /// steps ([`crate::vrchat_launch::way`]).
    #[serde(default)]
    pub several_profiles: bool,
    /// The place VRChat was started for without it, until it is delivered; or why it was not.
    #[serde(default)]
    pub joining: Option<Joining>,
    /// VRChat ran and its log could not be read: whose a VRChat is cannot be told here.
    #[serde(default)]
    pub log_trouble: Option<LogTrouble>,
}

/// A private place on its way to a VRChat that was started without it ([`LauncherState::joining`]): its world's name alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joining {
    /// The world's name; empty when it is not known.
    pub world: String,
    pub step: JoinStep,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinStep {
    /// VRChat is starting.
    #[default]
    Starting,
    /// VRChat is up: until its log says who is signed in.
    SigningIn,
    /// It was not delivered. The place is kept: [`Action::JoinAgain`].
    Failed(JoinFailure),
}

/// Why a place was not delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinFailure {
    /// VRChat is signed in with another account.
    OtherAccount,
    /// VRChat did not say in time who is signed in.
    NoProof,
    /// VRChat's log cannot be read.
    Log(LogTrouble),
    /// VRChat did not come up, or closed before it could be.
    Gone,
    /// Another start came in its place, or VRChat came up on another profile.
    Elsewhere,
    /// It did not go, in the person's words.
    NotSent(String),
}

impl JoinFailure {
    /// Whose the VRChat is could not be told: the way left is the account's own invite to itself, which is nobody else's.
    pub fn unproven(&self) -> bool {
        matches!(self, JoinFailure::NoProof | JoinFailure::Log(_))
    }
}

/// Why VRChat's log cannot be read ([`LauncherState::log_trouble`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogTrouble {
    /// VRChat's folder is not there.
    NoFolder,
    /// It may not be read: this folder or file.
    Denied(String),
    /// VRChat runs and writes none.
    NotWritten,
}

/// The engine's files no VRChat starts on, on a Mac ([`LauncherState::engine_trouble`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineTrouble {
    /// The files, by their path in the engine; none when they are not known.
    pub paths: Vec<String>,
    /// What ends it.
    pub action: crate::obelisk::EngineAction,
    /// It is being installed again now: how far, when that is known.
    #[serde(default)]
    pub repairing: bool,
    #[serde(default)]
    pub percent: Option<u8>,
}

/// A record of VRChat's engine on a Mac that cannot be read ([`LauncherState::start_trouble`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTrouble {
    /// The file, whole (or its folder, when that cannot be listed).
    pub file: String,
    /// The folder of the records cannot be listed: only its permissions can mend it.
    pub folder: bool,
    /// Play is held by it now.
    pub blocks: bool,
    /// How long until it may be set aside (it may still be a start that just ended); `None` once it may be, or never can.
    pub set_aside_in: Option<u64>,
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
        look: Look { rank, supporter: n.is_multiple_of(3), banner_colour: (!n.is_multiple_of(2)).then_some([40, 60 + n as u8 * 20, 160]), ..Look::default() },
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
        verified_for: None,
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
        kept: None,
        unresolved: 0,
        profiles: vec![
            GameProfile { profile: 0, account: Some("Sample Me".into()), ..GameProfile::default() },
            GameProfile { profile: 1, account: Some("Sample Alt".into()), ..GameProfile::default() },
        ],
        engine_setup: None,
        picture_lost: None,
        engine_trouble: None,
        start_trouble: None,
        several_profiles: true,
        joining: None,
        log_trouble: None,
        mine: Some(MyProfile {
            loaded: true,
            bio: "Made-up account for drawing the page.
Builds quiet worlds.".into(),
            banner: BannerChoice::Colour,
            banner_colour: Some([60, 90, 200]),
            supporter: true,
            ..MyProfile::default()
        }),
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
