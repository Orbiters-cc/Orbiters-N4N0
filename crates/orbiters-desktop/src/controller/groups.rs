//! VRChat groups for the window (REQUEST22 1; DESIGN22's revision after the attack; the fix round after
//! verify-r22-groups.md): Home's Groups section, the group page's reads and its single, guarded writes.
//!
//! - Nothing of a group goes in the snapshot (M9): every reply goes to the window that asked, on demand
//!   (`ToService::Groups` → `ToClient::Groups`). Only the window may ask; the overlay, voice and the generic commands
//!   cannot (M10). The service admits a few requests at once BEFORE starting a thread for one ([`Controller::groups_admit`]).
//! - Every request is bound to its [`Scope`] (environment, Orbiters account, the signed-in VRChat account, B3) and the
//!   ids it names; a cached account in "Connecting" grants nothing. The scope is checked again under the client's lock
//!   right before each write is dispatched, and before a reply goes out.
//! - Every write is prepared ([`Request::Prepare`]: fresh reads of the exact target, the rules of
//!   `orbiters_core::groups::can`, the exact payload) into one [`Sealed`] object, written in the [`Ledger`] as prepared,
//!   and committed once ([`Request::Commit`], with the digest of what the person was shown) by the same window. A window
//!   has one sealed write at a time: another prepare, a cancel, its link closing or another account ends it unsent. What
//!   goes out is the sealed payload and nothing of the window's.
//! - At the commit, under the client's lock: what decides it is read, the ledger is marked `Dispatched` on the disk, the
//!   write's pacing is waited for, and then, as one step with nothing else in it, the last read is made and the request
//!   made ready ([`Controller::groups_step`]). It is authorised at the effect (verify4 F1, F13): the HTTP client asks its
//!   [`Leave`] once, as it hands the request to the network, and [`Controller::groups_leave`] is the one place that gives
//!   it: the same Orbiters identity at the same epoch (the service's own counter, through N4N0's fence: an account is
//!   installed before, and nothing goes, or after the answer), the same verified scope, the sealed operation still on its
//!   way and used once, and what was just read as it was sealed (a role change's very membership with it).
//! - Its outcome is done, refused or UNKNOWN, kept by the ledger, by VRChat's own answer to the write alone. A check of an
//!   unknown one says what VRChat shows NOW (it is there, it is not, cannot tell), never what happened or who did it; it is
//!   closed when VRChat shows what was asked for, or by the person ([`Request::Decide`]), never by default (verify3
//!   Blocker 2).
//! - Posts are created and deleted here, never edited: VRChat's edit carries the audience whole and takes no condition
//!   (verify3 Blocker 4). A member's role change is bound to their membership (verify3 Blocker 1).
//! - One write at a time per target, a few requests at a time in all; the client's lock is let go between calls (M9,
//!   M11). No bulk, no background sweeps.
use orbiters_core::locks::*;
use super::vrchat_data::Scope;
use super::vrchat_login::Presence;
use super::Controller;
pub use super::groups_ledger::{Answer, By, Closing, Entry, Evidence, Key, Kind, Ledger, Not, State, Stop, Target, Warning};
use super::groups_ledger::{known_id, FORMAT, SETTLES_AFTER_MS};
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Authority, Beside, Catalogue, FileTag, Group, GroupCard, InventoryFile, Member, Membership, Missing, Part, PermissionInfo, Permit, Post, PostDraft, Role, RoleChange, RoleDraft, RoleEdit, RoleField, Why};
use orbiters_core::group_events::{CalendarEvent, Events, Month, Told, EVENTS_KEPT};
use orbiters_core::vrchat_api::{Client, GroupFailure, GroupRefusal, GroupFile, GroupInventory, Held, Leave, Members, Posts};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The link of the in-process window (no tray service between).
pub const LOCAL_LINK: u64 = 0;
/// A prepared write is committed within this time or prepared again.
const INTENT_FOR: Duration = Duration::from_secs(300);
/// Prepared writes kept at once (one per window), ids remembered as used.
const INTENTS_MAX: usize = 16;
const USED_MAX: usize = 128;
/// Staged uploads no prepared write is for any more go after this long.
const STAGED_FOR: Duration = Duration::from_secs(3600);
/// Entries of the ledger shown to the window at most, those not ended aside.
const SHOWN_MAX: usize = 200;
/// A post made this long before the write was prepared or earlier is none of its candidates (clocks differ).
const CLOCKS_DIFFER_MS: i64 = 10 * 60 * 1000;
/// Reads at once inside the controller (each lets the client's lock go between its calls).
const RUNNING_MAX: usize = 3;
/// Threads the service starts for groups requests at once, commits and cancels included (M9: admission before a thread).
pub const ADMIT_MAX: usize = 6;
/// Groups on Home at most, and those read one by one at discovery when the account's list does not show them (the
/// rest are read as their cards come into view, [`Request::Hydrate`], Major 7).
const GROUPS_MAX: usize = 1000;
const HYDRATE_MAX: usize = 12;
/// Groups a [`Request::Hydrate`] reads at most.
pub const HYDRATE_BATCH: usize = 8;
/// What a check offers the person at most.
const CANDIDATES_MAX: usize = 20;
/// Pages of posts read to find one post (a deletion's target, a check's candidates), and of inventory to find a file.
const POST_PAGES: usize = 4;
const INVENTORY_PAGES: usize = 4;
/// A picture of the page at most (memory only).
const PICTURE_MAX: usize = 12 * 1024 * 1024;
/// Groups and roles as they were before a write, kept to put back what moved beside it.
const WAS_MAX: usize = 16;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Home's Groups section: the groups this account owns or helps run.
    Discover,
    /// The cards of groups listed without their names yet (in view on Home), a few at a time.
    Hydrate { ids: Vec<String> },
    /// A group's page: the group with its roles and the account's membership, and (with rights over roles) the rights'
    /// catalogue.
    Open { group: String },
    /// A page of members (`query` of three characters or more: VRChat's search).
    Members { group: String, query: String, offset: usize },
    Posts { group: String, offset: usize },
    /// A page of a group's calendar, whole or a month of it (read only).
    Events { group: String, month: Option<Month>, offset: usize },
    /// A group's calendar as the window has it once looked at whole, and what Orbiters says of it, to show at the next start:
    /// nothing is asked of VRChat.
    KeepEvents { group: String, events: Vec<CalendarEvent>, told: Box<Told> },
    /// A page of the account's own pictures.
    Inventory { tag: FileTag, offset: usize },
    /// A picture of the page (a member's, a banner), in memory only.
    Picture { url: String },
    Prepare { write: Write },
    /// The picture of the sealed write `op`, as sealed: the bytes whose SHA-256 it holds.
    SealedArt { op: String },
    /// Send the sealed write `op`, shown to the person as `digest` says.
    Commit { op: String, digest: String },
    Cancel { op: String },
    /// The account's entries of the ledger: those open, those of `also`, and the newest others.
    Ledger { also: Vec<String> },
    /// What VRChat shows now of the write `op`: a fresh read of exactly what it touched.
    Check { op: String },
    /// The person decides, after a check was asked for.
    Decide { op: String, choice: Choice },
    /// The person looked on vrchat.com and goes on: records that cannot be read hold this account no more.
    SetAside,
    /// The person saw to what moved beside the write `op`.
    SeenTo { op: String },
}

/// What the person decides of a write no one can tell the end of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Choice {
    /// They looked: it is there (`made`: which of those shown, when it is one of them). Closed.
    Shows { made: Option<String> },
    /// They looked: it is not there. It stays open, to be sent again or dropped.
    ShowsNot,
    /// Again, as a new operation: closed, and the same may be sealed again, with a warning.
    Again,
    /// Left: closed, with the same warning for the same again.
    Drop,
}

/// A PNG the window put in the staging folder for an upload: only its name, size and SHA-256 cross the link (Major 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// The staging folder of uploads, under the local data folder (the test folders in tests).
pub fn staging_dir() -> std::path::PathBuf {
    orbiters_core::paths::local_data_root().join("group-uploads")
}

impl Staged {
    /// The file's name is one the window makes (a UUID): nothing else is ever read.
    fn path(&self) -> Option<std::path::PathBuf> {
        let ok = self.name.len() <= 64 && self.name.ends_with(".png") && self.name[..self.name.len() - 4].bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        ok.then(|| staging_dir().join(&self.name))
    }
    /// The bytes, when they are what was announced (size, hash, a PNG, within the limit).
    pub(crate) fn read(&self) -> Result<Vec<u8>, Failure> {
        use sha2::Digest;
        let path = self.path().ok_or(Failure::Api(GroupFailure::Invalid))?;
        if self.size as usize > orbiters_core::vrchat_api::GROUP_UPLOAD_MAX {
            return Err(Failure::TooLarge);
        }
        let len = std::fs::metadata(&path).map_err(|_| Failure::Expired)?.len();
        if len != self.size {
            return Err(Failure::Changed);
        }
        let bytes = std::fs::read(&path).map_err(|_| Failure::Expired)?;
        let hash: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if hash != self.sha256 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Failure::Changed);
        }
        Ok(bytes)
    }
}

/// One write, as the person asked for it, with the ids captured at their press (M3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Write {
    /// `membership`: the very one of the row it was asked on (verify4 F1).
    MemberRole { group: String, user: String, membership: Membership, role: String, add: bool },
    CreateRole { group: String, draft: RoleDraft },
    /// A role's words and flags: not its rights (`RoleEdit`).
    /// `back`: the write it puts right, whose answer showed these parts moved beside it.
    EditRole {
        group: String,
        baseline: Role,
        edit: RoleEdit,
        #[serde(default)]
        back: Option<String>,
    },
    DeleteRole { group: String, role: String },
    /// `of`: the new post being written it comes from (an id of the window's), cleared when this very one is done.
    CreatePost { group: String, draft: PostDraft, of: String },
    /// `post` is the post as the person saw it: a different one now is not deleted (Blocker 7).
    DeletePost { group: String, post: Post },
    /// A PNG into the account's own gallery or icons, for `group`: a step of its own (M6).
    Upload {
        tag: FileTag,
        group: String,
        staged: Staged,
        /// The studio's art it was made from, as a number (none for an icon).
        #[serde(default)]
        art: Option<u64>,
    },
    /// `back`: the write it puts right, whose answer showed that slot's picture gone: the file is the one the service read
    /// there just before (it need not be the account's own).
    ApplyArt {
        group: String,
        slot: ArtSlot,
        file: String,
        #[serde(default)]
        back: Option<String>,
    },
}

impl Write {
    /// What one write at a time is counted on.
    fn target(&self) -> String {
        match self {
            Write::MemberRole { group, user, .. } => format!("member:{group}:{user}"),
            Write::CreateRole { group, .. } | Write::EditRole { group, .. } | Write::DeleteRole { group, .. } => format!("roles:{group}"),
            Write::CreatePost { group, .. } | Write::DeletePost { group, .. } => format!("posts:{group}"),
            Write::ApplyArt { group, .. } => format!("art:{group}"),
            Write::Upload { tag, .. } => format!("upload:{}", tag.wire()),
        }
    }
    pub fn kind(&self) -> Kind {
        match self {
            Write::MemberRole { add: true, .. } => Kind::AssignRole,
            Write::MemberRole { add: false, .. } => Kind::UnassignRole,
            Write::CreateRole { .. } => Kind::CreateRole,
            Write::EditRole { .. } => Kind::EditRole,
            Write::DeleteRole { .. } => Kind::DeleteRole,
            Write::CreatePost { .. } => Kind::CreatePost,
            Write::DeletePost { .. } => Kind::DeletePost,
            Write::ApplyArt { slot: ArtSlot::Banner, .. } => Kind::ApplyBanner,
            Write::ApplyArt { slot: ArtSlot::Icon, .. } => Kind::ApplyIcon,
            Write::Upload { tag: FileTag::Gallery, .. } => Kind::UploadGallery,
            Write::Upload { tag: FileTag::Icon, .. } => Kind::UploadIcon,
        }
    }
    /// The ids it is about.
    fn ids(&self) -> Target {
        let mut t = Target::default();
        match self {
            Write::MemberRole { user, role, .. } => (t.user, t.role) = (Some(user.clone()), Some(role.clone())),
            Write::EditRole { baseline, .. } => t.role = Some(baseline.id.clone()),
            Write::DeleteRole { role, .. } => t.role = Some(role.clone()),
            Write::DeletePost { post, .. } => t.post = Some(post.id.clone()),
            Write::ApplyArt { file, .. } => t.file = Some(file.clone()),
            Write::CreateRole { .. } | Write::CreatePost { .. } | Write::Upload { .. } => {}
        }
        t
    }
    /// What goes over the wire, hashed: method, path and body (an upload's bytes by their own hash).
    fn payload(&self) -> String {
        use orbiters_core::vrchat_api as api;
        let (method, path, body) = match self {
            Write::MemberRole { group, user, role, add, .. } => (if *add { "PUT" } else { "DELETE" }, format!("{group}/members/{user}/roles/{role}"), String::new()), // i18n: not shown
            Write::CreateRole { group, draft } => ("POST", format!("{group}/roles"), api::role_create_body(draft)), // i18n: not shown
            Write::EditRole { group, baseline, edit, .. } => ("PUT", format!("{group}/roles/{}", baseline.id), api::role_update_body(&RoleChange::between(baseline, edit))), // i18n: not shown
            Write::DeleteRole { group, role } => ("DELETE", format!("{group}/roles/{role}"), String::new()), // i18n: not shown
            Write::CreatePost { group, draft, .. } => ("POST", format!("{group}/posts"), api::post_create_body(draft)), // i18n: not shown
            Write::DeletePost { group, post } => ("DELETE", format!("{group}/posts/{}", post.id), String::new()), // i18n: not shown
            Write::ApplyArt { group, slot, file, .. } => ("PUT", group.clone(), api::art_body(*slot, file)), // i18n: not shown
            Write::Upload { tag, staged, .. } => ("POST", format!("file/image/{}", tag.wire()), staged.sha256.clone()), // i18n: not shown
        };
        g::digest(&[method, &path, &body])
    }
    /// What makes it the same again, hashed: the same people, roles, rights, words, audience and pictures, whatever their
    /// spaces, Unicode form and order (its group and kind are the entry's own).
    fn same(&self) -> String {
        match self {
            Write::MemberRole { user, role, add, .. } => g::digest(&["member-role", user, role, if *add { "add" } else { "remove" }]),
            Write::CreateRole { draft, .. } => g::digest(&["create-role", &draft.canonical()]),
            Write::EditRole { baseline, edit, .. } => g::digest(&["edit-role", &baseline.id, &RoleChange::between(baseline, edit).canonical()]),
            Write::DeleteRole { role, .. } => g::digest(&["delete-role", role]),
            Write::CreatePost { draft, .. } => g::digest(&["create-post", &draft.canonical()]),
            Write::DeletePost { post, .. } => g::digest(&["delete-post", &post.id]),
            Write::ApplyArt { slot, file, .. } => g::digest(&["apply", &format!("{slot:?}"), file]),
            Write::Upload { tag, staged, .. } => g::digest(&["upload", tag.wire(), &staged.sha256]),
        }
    }
    /// The parts of the group or role it sets, of those a write can move.
    pub fn parts(&self) -> Vec<Part> {
        match self {
            Write::ApplyArt { slot, .. } => vec![slot.part()],
            Write::EditRole { baseline, edit, .. } => RoleChange::between(baseline, edit).fields().into_iter().map(RoleField::part).collect(),
            _ => Vec::new(),
        }
    }
    /// What its permit is for.
    fn subject(&self) -> String {
        match self {
            Write::MemberRole { user, membership, role, add, .. } => Action::AssignRole { role, user, membership, add: *add }.subject(),
            Write::CreateRole { draft, .. } => Action::CreateRole { draft }.subject(),
            Write::EditRole { baseline, edit, .. } => Action::EditRole { role: &baseline.id, change: &RoleChange::between(baseline, edit) }.subject(),
            Write::DeleteRole { role, .. } => Action::DeleteRole { role }.subject(),
            Write::CreatePost { .. } | Write::DeletePost { .. } => Action::Post.subject(),
            Write::ApplyArt { .. } => Action::ApplyArt.subject(),
            Write::Upload { .. } => g::UPLOAD.into(),
        }
    }
    /// The write it puts right, when it is one that does.
    pub fn back(&self) -> Option<&str> {
        match self {
            Write::EditRole { back, .. } | Write::ApplyArt { back, .. } => back.as_deref(),
            _ => None,
        }
    }
    pub fn group(&self) -> &str {
        match self {
            Write::MemberRole { group, .. }
            | Write::CreateRole { group, .. }
            | Write::EditRole { group, .. }
            | Write::DeleteRole { group, .. }
            | Write::CreatePost { group, .. }
            | Write::DeletePost { group, .. }
            | Write::Upload { group, .. }
            | Write::ApplyArt { group, .. } => group,
        }
    }
}

/// A picture sealed in a write: the file it is (an apply's) and its bytes, by their hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedArt {
    pub file: Option<String>,
    pub sha256: String,
    pub size: u64,
    /// The file's version it is (an apply's): another one by the send, and it does not go.
    pub version: Option<u64>,
}

/// A write prepared and sealed (verify2 B-E, B-F): everything that will be sent, and everything its confirmation shows,
/// as the service read it. The window draws the confirmation from this and nothing else, and sends back the digest of
/// what it drew; the service sends its own copy. Nothing can be put in it, changed in it, or pointed elsewhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sealed {
    pub op: String,
    pub group: String,
    /// The group's name, as read just now.
    pub group_name: String,
    pub write: Write,
    /// Public, destructive or privilege-bearing (the permit's word), or warned of: the window shows it and waits.
    pub confirm: bool,
    /// The role concerned, as read just now (a member's role change, a role's change or deletion).
    pub role: Option<Role>,
    /// The post concerned, as read just now (a deletion).
    pub post: Option<Post>,
    /// The member concerned, as read just now, with the very membership (its id and when it began): ids that tell
    /// namesakes apart, and one who left and came back from who they were.
    pub member: Option<Member>,
    pub art: Option<SealedArt>,
    /// Names to show for the roles and rights it names (id or key, label).
    pub labels: Vec<(String, String)>,
    pub warning: Option<Warning>,
    digest: String,
}

impl Sealed {
    /// The digest of what this holds now (not the one written in it).
    pub fn digest(&self) -> String {
        let text = serde_json::to_string(&Sealed { digest: String::new(), ..self.clone() }).unwrap_or_default();
        g::digest(&[&text])
    }
    fn seal(mut self) -> Self {
        self.digest = self.digest();
        self
    }
    /// Made up, for the screenshot review and tests of drawing.
    #[doc(hidden)]
    pub fn made_up(write: Write, confirm: bool) -> Self {
        Self { op: "op".into(), group: write.group().into(), group_name: String::new(), write, confirm, role: None, post: None, member: None, art: None, labels: Vec::new(), warning: None, digest: String::new() }.seal()
    }
    pub fn label(&self, id: &str) -> Option<&str> {
        self.labels.iter().find(|(i, _)| i == id).map(|(_, l)| l.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Done {
    /// The member's roles as VRChat now has them (the committed state, M3).
    MemberRoles { user: String, role_ids: Vec<String> },
    Role(Role),
    Roles(Vec<Role>),
    Post(Post),
    PostDeleted { post: String },
    Uploaded(InventoryFile),
    Group(Box<Group>),
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Done(Done),
    /// Nothing was done: why.
    Refused(Failure),
    /// It was sent and its answer was lost: it may have been done. Nothing is retried; check before sending again.
    Unknown,
}

/// How much something VRChat shows looks like what a write asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Likeness {
    /// Exactly what was asked for.
    Exactly,
    NotExactly,
    /// It was not there before, of the kind asked (a picture in other bytes: VRChat may keep others than it was given).
    Since,
    /// Nothing says when it came, or of what kind it is.
    Uncertain,
}

/// Something VRChat shows that the write may have made: its id, its words or address (shown, never kept), when, and how
/// much it looks like what was asked. It says nothing of who made it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    /// A picture's address.
    pub url: String,
    pub at: Option<String>,
    pub like: Likeness,
}

/// Why a check cannot tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unclear {
    /// It went out moments ago: VRChat may still be at it.
    TooSoon,
    /// What VRChat answered is not of this very object, or not whole.
    NotRead,
    /// Not seen, which says nothing here.
    NotSeen,
    /// The person left the group and joined again: what VRChat shows is of another membership.
    Rejoined,
    /// There is no record of it here, or none that can be read.
    NoRecord,
    /// VRChat could not be asked.
    Failed(Failure),
}

/// What a check found: what VRChat shows now, not what happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Observation {
    /// VRChat answered the write itself at the time (done, or its "no"), or it stopped before it went: nothing was read.
    Answered { done: bool },
    /// It is being sent now.
    Running,
    /// VRChat, just read, shows what was asked for, whoever did it. Closed.
    Shows,
    /// VRChat does not show it now: it may never have applied, or was changed since.
    ShowsNot,
    /// What is there that it may have made, for the person: it cannot tell.
    Candidates(Vec<Candidate>),
    CannotTell(Unclear),
    /// It was settled before (by a check, or by the person): nothing was read now (verify4 F4).
    Closed,
}

/// Why a request gave nothing: local codes, worded by the window. No server text, no member, role or post text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    Api(GroupFailure),
    /// Not signed in to VRChat for certain (signed out, or still connecting).
    SignedOut,
    /// The Orbiters or VRChat account (or the environment) changed since this was asked.
    AccountChanged,
    /// Enough is being asked already: in a moment.
    Busy,
    /// The rules refuse it, and why.
    Withheld(Why),
    /// VRChat's answer left out what decides it: nothing is guessed (read again).
    NotRead(Missing),
    /// The prepared write is gone (too old, cancelled, the account changed): prepare it again.
    Expired,
    /// This prepared write was already committed once.
    Duplicate,
    /// What it was made against changed since (the member's roles, the picture, what is there).
    Changed,
    /// The person left the group and joined again since it was looked at: another membership, not the one decided for.
    Rejoined,
    /// Someone changed the post since the person saw it, and these parts: nothing was sent.
    PostChanged(g::PostParts),
    /// Someone changed the role since the person saw it, and these parts: nothing was sent.
    RoleChanged(g::RoleParts),
    /// This very write may already have been sent: check first.
    MaySent,
    /// Another like it went out since this one was shown: it is to be looked at again.
    Since,
    /// A record of an earlier write cannot be read: it may be of this.
    RecordUnreadable,
    /// Another Orbiters on this computer keeps the record: it is done there.
    OtherOwner,
    /// The record of the write could not be kept on this computer: nothing was sent.
    NotKept,
    /// What a check saw, or what the person decided, could not be kept: the write it is of stays as it was (it may have
    /// been done).
    LookNotKept,
    /// Only the window asks this.
    NotHere,
    /// Over the size this accepts.
    TooLarge,
    /// The service's answer did not arrive (the link closed or timed out): whatever was asked may have been done.
    Lost,
    /// The service could not be reached: nothing was asked of it.
    Offline,
}

/// Each card asked, or why it could not be read.
pub type Hydrated = Vec<(String, Result<GroupCard, Failure>)>;

/// The group's page as read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub group: Group,
    /// The rights a role can hold: read only with rights over roles.
    pub catalogue: Option<Vec<PermissionInfo>>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Discovered(Vec<GroupCard>),
    /// Each card asked, or why it could not be read: one that fails leaves the others (J8).
    Hydrated(Hydrated),
    Opened(Box<Page>),
    Members(Members),
    Posts(Posts),
    Events(Events),
    Inventory(GroupInventory),
    Picture(#[serde(with = "super::vrchat_login::bytes_b64")] Vec<u8>),
    Prepared(Box<Sealed>),
    /// `said`: what VRChat itself answered, when it is VRChat that refused.
    Outcome { op: String, outcome: Outcome, #[serde(default)] said: Option<GroupRefusal> },
    /// This very write may already have been sent: that one.
    MaySent { op: String },
    /// The account's entries, how many records cannot be read that may be its own, and what can be put back from here of
    /// what moved beside a write (its `beside`), each a write of its own to be confirmed.
    Ledger { entries: Vec<Entry>, unreadable: usize, back: Vec<(String, Write)> },
    Checked { op: String, observation: Observation },
    /// What the person decided is kept.
    Decided { op: String },
    Cancelled,
    Failed(Failure),
}

struct Intent {
    scope: Scope,
    link: u64,
    sealed: Sealed,
    /// The picture's bytes (an apply's).
    picture: Option<Vec<u8>>,
    /// Where the post was found in the list.
    offset: usize,
    /// The file applied, as read, and where (an apply's).
    file: Option<Filed>,
    /// What was in the slot (an apply's).
    previous: Option<String>,
    /// The ids there before (hashed), and whether that was all of them.
    before: Vec<String>,
    whole: bool,
    /// The entries of the ledger its person was warned of.
    told: Vec<String>,
    at: Instant,
}

/// A file of the account's as read: by its id, or on a page of the list of its kind (what VRChat gives by its id, it may
/// not give to everyone).
#[derive(Clone)]
struct Filed {
    detail: GroupFile,
    page: Option<usize>,
}

/// The service's side of groups: the sealed writes waiting, the ledger, targets being written.
#[derive(Default)]
pub(crate) struct Runtime {
    intents: Vec<Intent>,
    /// The last prepare each window asked for, by its link: an earlier one still under way seals nothing, nor one whose link
    /// closed.
    prepares: HashMap<u64, u64>,
    turns: u64,
    used: VecDeque<String>,
    pub(crate) ledger: Arc<Ledger>,
    /// What the last check of each operation offered the person.
    candidates: HashMap<String, Vec<String>>,
    /// What a role or a group was just before a write that may have moved more of it than was asked: to put it back.
    was: VecDeque<Was>,
    writing: HashSet<String>,
    running: usize,
    #[cfg(test)]
    pub(crate) sent_ops: u64,
    /// Requests that reached the controller (the service's admission is tested by it).
    #[cfg(test)]
    pub(crate) entered: u64,
}

/// What was there just before a write, in memory only.
#[derive(Clone)]
struct Was {
    op: String,
    key: Key,
    what: Before,
}
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
enum Before {
    Group(Box<Group>),
    /// The role before, and as VRChat answered it.
    Role { before: Role, after: Option<Role> },
}

impl Was {
    /// What can be put back from here of `moved`: a picture of the other slot that was there (never the nameplate's, which
    /// this version does not send), a role's words and flags (never its rights, nor its place).
    fn back(&self, e: &Entry) -> Vec<Write> {
        let back = Some(e.op.clone());
        match &self.what {
            Before::Group(g) => [(Part::Banner, ArtSlot::Banner, &g.banner_id), (Part::Icon, ArtSlot::Icon, &g.icon_id)]
                .into_iter()
                .filter(|(part, _, _)| e.beside.moved.contains(part))
                .filter_map(|(_, slot, file)| Some(Write::ApplyArt { group: e.group.clone(), slot, file: file.clone()?, back: back.clone() }))
                .collect(),
            Before::Role { before, after: Some(after) } => {
                let edit = RoleEdit::back(before, after, &e.beside.moved);
                edit.map(|edit| Write::EditRole { group: e.group.clone(), baseline: after.clone(), edit, back }).into_iter().collect()
            }
            Before::Role { .. } => Vec::new(),
        }
    }
}

impl Runtime {
    /// Sealed writes ended unsent: in the ledger as never sent, their staged picture gone (J3).
    fn end_unsent(&mut self, gone: impl Fn(&Intent) -> bool) {
        let (out, kept) = std::mem::take(&mut self.intents).into_iter().partition(|i| gone(i));
        self.intents = kept;
        let out: Vec<Intent> = out;
        for i in out {
            self.ledger.never_sent(&i.sealed.op);
            drop_staged(&i.sealed.write);
        }
    }
}

fn drop_staged(write: &Write) {
    if let Write::Upload { staged, .. } = write {
        if let Some(p) = staged.path() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// A request the service did not take in (too many, not the window's, its link closed before): the picture staged for it
/// goes with it (verify4 F9).
pub fn not_taken_in(request: &Request) {
    if let Request::Prepare { write } = request {
        drop_staged(write);
    }
}

/// The requests the window may make, and nobody else (M10).
pub fn allowed_from(role: crate::ipc::Role) -> bool {
    role == crate::ipc::Role::Window
}

/// A groups request the service took in: counted until its thread ends.
pub struct Admitted(Arc<AtomicUsize>);
impl Drop for Admitted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One running request, counted while it runs.
struct Slot<'a>(&'a Controller);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut rt = self.0.groups.locked();
        rt.running = rt.running.saturating_sub(1);
    }
}

/// The rules on a freshly read group: the permit, or why not. What is missing is never allowed.
fn authority(action: &Action, group: &Group, me: &str, catalogue: Option<&[PermissionInfo]>) -> Result<Permit, Failure> {
    let cat = Catalogue { roles: group.roles.as_deref(), permissions: catalogue };
    permit(g::can(*action, Actor::of(group, me), &cat))
}

fn permit(a: Authority) -> Result<Permit, Failure> {
    match a {
        Authority::Allowed(p) => Ok(p),
        Authority::Refused(why) => Err(Failure::Withheld(why)),
        Authority::Unknown(m) => Err(Failure::NotRead(m)),
    }
}

/// A read's result inside the client's lock.
fn read<T>(r: Result<T, GroupFailure>) -> Result<T, Failure> {
    r.map_err(Failure::Api)
}

/// The same role for the purpose of a write made against `then`: its rights, rank, words and flags unchanged.
fn same_role(then: &Role, now: &Role) -> Result<(), Failure> {
    let parts = then.differs(now);
    if parts.any() { Err(Failure::RoleChanged(parts)) } else { Ok(()) }
}

/// The post as the person saw it, or what of it is not.
fn same_post(then: &Post, now: &Post) -> Result<(), Failure> {
    if g::same_post(then, now) { Ok(()) } else { Err(Failure::PostChanged(then.differs(now))) }
}

impl Controller {
    /// Groups requests that reached the controller so far (tests of the service's admission).
    #[cfg(test)]
    pub(crate) fn groups_book_for_tests(&self) -> Arc<Ledger> {
        self.groups_book()
    }
    /// The record an earlier program left in `dir`.
    #[cfg(test)]
    pub(crate) fn groups_keep_book_for_tests(&self, dir: std::path::PathBuf) {
        self.groups.lock().unwrap().ledger = Arc::new(Ledger::at(dir));
    }
    #[cfg(test)]
    pub(crate) fn groups_entered(&self) -> u64 {
        self.groups.lock().unwrap().entered
    }

    /// The service takes a groups request in only while fewer than [`ADMIT_MAX`] run: checked before its thread starts.
    pub fn groups_admit(&self) -> Option<Admitted> {
        let n = self.groups_admitted.clone();
        let mut now = n.load(Ordering::Acquire);
        loop {
            if now >= ADMIT_MAX {
                return None;
            }
            match n.compare_exchange(now, now + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Admitted(n)),
                Err(seen) => now = seen,
            }
        }
    }

    /// A request of the window's (link `link`) under `scope`. Blocking: call on a thread of its own.
    pub fn groups(&self, link: u64, scope: Scope, request: Request) -> Reply {
        #[cfg(test)]
        {
            self.groups.lock().unwrap().entered += 1;
        }
        // A staged picture goes with the request it came with, whatever refuses that (J3).
        let refused = |f: Failure| {
            if let Request::Prepare { write } = &request {
                drop_staged(write);
            }
            Reply::Failed(f)
        };
        if scope != Scope::of(&self.view()) {
            return refused(Failure::AccountChanged);
        }
        if self.view().vrchat_login.presence() != Presence::SignedIn {
            return refused(Failure::SignedOut);
        }
        if !self.groups_live(&scope) {
            return refused(Failure::AccountChanged);
        }
        {
            let mut rt = self.groups.locked();
            // What is sealed under another scope, or too old, ends unsent (B3).
            rt.end_unsent(|i| i.scope != scope || i.at.elapsed() >= INTENT_FOR);
            let key = Key::of(&scope);
            rt.was.retain(|w| Some(&w.key) == key.as_ref());
            // Committing, cancelling, the ledger, checking and saying never wait behind reads.
            let control = matches!(request, Request::Cancel { .. } | Request::Commit { .. } | Request::Ledger { .. } | Request::Check { .. } | Request::Decide { .. } | Request::SetAside | Request::SeenTo { .. });
            if rt.running >= RUNNING_MAX && !control {
                drop(rt);
                return refused(Failure::Busy);
            }
            rt.running += 1;
        }
        let _slot = Slot(self);
        let (turn, asked) = (self.kept_turn(), matches!(request, Request::Discover | Request::Hydrate { .. } | Request::Open { .. } | Request::Members { .. } | Request::Posts { .. } | Request::Events { .. } | Request::KeepEvents { .. }).then(|| request.clone()));
        let reply = match request {
            Request::Discover => self.groups_discover(&scope).map(Reply::Discovered),
            Request::Hydrate { ids } => self.groups_hydrate(&scope, ids).map(Reply::Hydrated),
            Request::Open { group } => self.groups_open(&scope, &group).map(|p| Reply::Opened(Box::new(p))),
            Request::Ledger { also } => self.groups_ledger(&scope, &also),
            Request::Check { op } => self.groups_check(link, &scope, &op).map(|observation| Reply::Checked { op, observation }),
            Request::Decide { op, choice } => self.groups_decide(&scope, &op, choice).map(|_| Reply::Decided { op }),
            Request::SetAside => self.groups_key(&scope).and_then(|key| self.groups_book().set_aside(&key).map_err(not)).map(|_| Reply::Cancelled),
            Request::SeenTo { op } => self.groups_key(&scope).and_then(|key| if self.groups_book().seen_to(&key, &op) { Ok(Reply::Decided { op }) } else { Err(Failure::LookNotKept) }),
            Request::SealedArt { op } => self.groups_sealed_art(link, &scope, &op).map(Reply::Picture),
            Request::Members { group, query, offset } => {
                let query = query.trim().to_string();
                self.groups_read(&scope, |c| if query.chars().count() >= 3 { c.search_group_members(&group, &query, offset) } else { c.group_members(&group, offset) }).map(Reply::Members)
            }
            Request::Posts { group, offset } => self.groups_read(&scope, |c| c.group_posts(&group, offset)).map(Reply::Posts),
            Request::Events { group, month, offset } => self.groups_read(&scope, |c| c.group_events(&group, month, offset)).map(Reply::Events),
            Request::KeepEvents { group, events, told } => if g::is_group_id(&group) && events.len() <= EVENTS_KEPT && told.fits() { Ok(Reply::Cancelled) } else { Err(Failure::TooLarge) },
            Request::Inventory { tag, offset } => self.groups_read(&scope, |c| c.inventory(tag, offset)).map(Reply::Inventory),
            Request::Picture { url } => self
                .groups_read(&scope, |c| c.image(&url).map_err(|e| if e == orbiters_core::vrchat_api::Error::UnsafeUrl { GroupFailure::NotFound } else { GroupFailure::Unavailable }))
                .and_then(|bytes| if bytes.len() > PICTURE_MAX { Err(Failure::TooLarge) } else { Ok(Reply::Picture(bytes)) }),
            Request::Prepare { write } => self.groups_prepare(link, &scope, write),
            Request::Commit { op, digest } => {
                let (outcome, said) = self.groups_commit(link, &scope, &op, &digest);
                // The ledger keeps how it ended; another account is told only that it is not its to know (B3).
                let (outcome, said) = if self.groups_live(&scope) { (outcome, said) } else { (Outcome::Unknown, None) };
                return Reply::Outcome { op, outcome, said };
            }
            Request::Cancel { op } => {
                self.groups.locked().end_unsent(|i| i.sealed.op == op && i.link == link);
                Ok(Reply::Cancelled)
            }
        };
        // Nothing read for one account is handed to another (B3): by the service's own epoch, not only by what it published
        // (verify4 F13).
        if !self.groups_live(&scope) {
            return Reply::Failed(Failure::AccountChanged);
        }
        // What the page shows at the next start (REQUEST28).
        if let (Some(request), Ok(reply)) = (&asked, &reply) {
            self.kept_groups(turn, &scope, request, reply);
        }
        reply.unwrap_or_else(Reply::Failed)
    }

    /// Whether `scope` is of whoever is signed in now: the identity the service has installed (its epoch, whatever it has
    /// published yet), and what it publishes of them.
    fn groups_live(&self, scope: &Scope) -> bool {
        scope.epoch() == Some(self.identity_epoch()) && *scope == Scope::of(&self.view())
    }

    /// A window's link closed: what it had sealed and not sent is over.
    pub fn groups_link_closed(&self, link: u64) {
        let mut rt = self.groups.locked();
        rt.prepares.remove(&link);
        rt.end_unsent(|i| i.link == link);
    }

    /// Whose the record's entries are ([`Key::of`]): all of it the scope's own, never of whoever is signed in by now.
    pub(crate) fn groups_key(&self, scope: &Scope) -> Result<Key, Failure> {
        Key::of(scope).ok_or(Failure::SignedOut)
    }
    fn groups_book(&self) -> Arc<Ledger> {
        self.groups.locked().ledger.clone()
    }

    /// What the window shows of the ledger: ids and states, no text. (Asked at each page: what an ended program left staged
    /// goes here too.)
    fn groups_ledger(&self, scope: &Scope, also: &[String]) -> Result<Reply, Failure> {
        let key = self.groups_key(scope)?;
        self.groups_sweep_staged();
        let book = self.groups_book();
        if !book.owned() {
            return Err(Failure::OtherOwner);
        }
        let unreadable = book.unreadable(&key);
        let mut left = SHOWN_MAX;
        let entries: Vec<Entry> = book
            .of(&key)
            .into_iter()
            .filter(|e| {
                e.open() || e.beside.any() || also.contains(&e.op) || {
                    left = left.saturating_sub(1);
                    left > 0
                }
            })
            .collect();
        let was: Vec<Was> = self.groups.locked().was.iter().filter(|w| w.key == key).cloned().collect();
        let back = entries.iter().filter(|e| !e.open() && e.beside.any()).flat_map(|e| was.iter().filter(|w| w.op == e.op).flat_map(|w| w.back(e)).map(|w| (e.op.clone(), w))).collect();
        Ok(Reply::Ledger { entries, unreadable, back })
    }

    /// A read of the signed-in account, the scope checked under the client's lock before and after.
    fn groups_read<T>(&self, scope: &Scope, f: impl FnOnce(&mut Client) -> Result<T, GroupFailure>) -> Result<T, Failure> {
        let user = scope_user(scope)?;
        let source = self.vrchat_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        match source.personal_until(&user, || self.groups_live(scope), deadline, |c| Ok(f(c))) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(Failure::Api(e)),
            Err(orbiters_core::vrchat_api::Error::Disabled) => Err(Failure::AccountChanged),
            Err(_) => Err(Failure::SignedOut),
        }
    }

    fn groups_discover(&self, scope: &Scope) -> Result<Vec<GroupCard>, Failure> {
        let me = scope_user(scope)?;
        let rights = self.groups_read(scope, |c| c.my_group_permissions())?;
        let listed = self.groups_read(scope, |c| c.my_groups())?;
        let mut cards: Vec<GroupCard> = Vec::new();
        for (id, permissions) in &rights {
            let shown = listed.iter().find(|(card, _)| card.id == *id);
            let owner = shown.is_some_and(|(_, owner)| *owner == me) || permissions.iter().any(|p| p == g::ALL);
            if !g::listed(permissions, owner) {
                continue;
            }
            let mut card = shown.map(|(card, _)| card.clone()).unwrap_or_else(|| GroupCard { id: id.clone(), ..Default::default() });
            card.owner = owner;
            card.permissions = permissions.clone();
            cards.push(card);
        }
        // Owned groups the rights' map left out still show.
        for (card, owner) in &listed {
            if *owner == me && !cards.iter().any(|c| c.id == card.id) {
                cards.push(GroupCard { owner: true, permissions: vec![g::ALL.into()], ..card.clone() });
            }
        }
        // Memberships the account's list does not show (hidden ones): a few read now, the rest as their cards come into
        // view; every one is listed (Major 7).
        for card in cards.iter_mut().filter(|c| !c.named).take(HYDRATE_MAX) {
            if let Ok(group) = self.groups_read(scope, |c| c.group(&card.id)) {
                fill_card(card, &group, &me);
            }
        }
        cards.sort_by(|a, b| b.owner.cmp(&a.owner).then_with(|| b.named.cmp(&a.named)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        cards.truncate(GROUPS_MAX);
        Ok(cards)
    }

    /// The cards of groups listed without a name yet, read one by one (a few at a time, as they come into view).
    fn groups_hydrate(&self, scope: &Scope, ids: Vec<String>) -> Result<Hydrated, Failure> {
        let me = scope_user(scope)?;
        if ids.len() > HYDRATE_BATCH || ids.iter().any(|id| !g::is_group_id(id)) {
            return Err(Failure::Api(GroupFailure::Invalid));
        }
        Ok(ids
            .into_iter()
            .map(|id| {
                let card = self.groups_read(scope, |c| c.group(&id)).map(|group| {
                    let mut card = GroupCard { id: id.clone(), ..Default::default() };
                    card.permissions = group.my_member.as_ref().map(|m| m.permissions.clone()).unwrap_or_default();
                    fill_card(&mut card, &group, &me);
                    card
                });
                (id, card)
            })
            .collect())
    }

    fn groups_open(&self, scope: &Scope, group: &str) -> Result<Page, Failure> {
        let found = self.groups_read(scope, |c| c.group(group))?;
        let me = found.my_member.as_ref().ok_or(Failure::Api(GroupFailure::Forbidden))?;
        // The rights' names, and which can be given: only a new role needs them. Without them the group is shown, and what
        // needs none of them goes on (verify4 F12).
        let catalogue = if g::abilities(&me.permissions).roles {
            match self.groups_read(scope, |c| c.group_permission_catalogue(group)) {
                Err(f @ (Failure::AccountChanged | Failure::SignedOut)) => return Err(f),
                read => read.ok().flatten(),
            }
        } else {
            None
        };
        Ok(Page { group: found, catalogue })
    }

    /// The post `id` as VRChat has it now, from the newest pages, and where its page starts.
    fn groups_find_post(&self, scope: &Scope, group: &str, id: &str) -> Result<Option<(Post, usize)>, Failure> {
        let mut offset = 0;
        for _ in 0..POST_PAGES {
            let page = self.groups_read(scope, |c| c.group_posts(group, offset))?;
            if let Some(p) = page.items.iter().find(|p| p.id == id) {
                return Ok(Some((p.clone(), offset)));
            }
            if page.raw < orbiters_core::vrchat_api::GROUP_PAGE || page.total.is_some_and(|t| (offset + page.raw) as u32 >= t) {
                break;
            }
            offset += page.raw;
        }
        Ok(None)
    }

    /// The account's own pictures of `tag`: those that can be told apart, the ids of all that were seen, and whether that is
    /// every one of them (each gave its id, and the list was read to its end: verify4 F2).
    fn groups_inventory(&self, scope: &Scope, tag: FileTag) -> Result<(Vec<GroupFile>, Vec<String>, bool), Failure> {
        let (mut out, mut ids, mut whole) = (Vec::new(), Vec::new(), true);
        let mut offset = 0;
        for _ in 0..INVENTORY_PAGES {
            let page = self.groups_read(scope, |c| c.inventory(tag, offset))?;
            offset += page.raw;
            out.extend(page.details);
            ids.extend(page.ids);
            whole &= page.whole;
            if page.raw < orbiters_core::vrchat_api::GROUP_PAGE {
                return Ok((out, ids, whole));
            }
        }
        Ok((out, ids, false))
    }

    /// The file `id`: by its id, or, when VRChat does not give it so, on the first pages of the account's pictures of its
    /// kind. One not found on a longer list is "not read", not "not yours" (N2).
    fn groups_file(&self, scope: &Scope, tag: FileTag, id: &str) -> Result<Filed, Failure> {
        match self.groups_read(scope, |c| c.picture_file(id)) {
            Ok(detail) => return Ok(Filed { detail, page: None }),
            Err(Failure::Api(GroupFailure::NotFound | GroupFailure::Forbidden)) => {}
            Err(f) => return Err(f),
        }
        let mut offset = 0;
        for _ in 0..INVENTORY_PAGES {
            let page = self.groups_read(scope, |c| c.inventory(tag, offset))?;
            if let Some(detail) = page.details.into_iter().find(|d| d.file.id == id) {
                return Ok(Filed { detail, page: Some(offset) });
            }
            if page.raw < orbiters_core::vrchat_api::GROUP_PAGE {
                return Err(Failure::Withheld(Why::NotYourPicture));
            }
            offset += page.raw;
        }
        Err(Failure::NotRead(Missing::Picture))
    }

    /// Fresh reads and the rules, sealed, written in the ledger as prepared, kept here under a one-use id.
    fn groups_prepare(&self, link: u64, scope: &Scope, write: Write) -> Result<Reply, Failure> {
        // A window has one sealed write at a time: whatever it had is over (B-E, B-F).
        let turn = {
            let mut rt = self.groups.locked();
            rt.end_unsent(|i| i.link == link);
            rt.turns += 1;
            let turn = rt.turns;
            rt.prepares.insert(link, turn);
            turn
        };
        self.groups_sweep_staged();
        let fail = write.clone();
        self.groups_seal(link, turn, scope, write).inspect_err(|_| drop_staged(&fail))
    }

    fn groups_seal(&self, link: u64, turn: u64, scope: &Scope, write: Write) -> Result<Reply, Failure> {
        let key = self.groups_key(scope)?;
        let ready = self.groups_validate(scope, write)?;
        let book = self.groups_book();
        let op = uuid::Uuid::new_v4().to_string();
        let mut entry = Entry {
            v: FORMAT,
            op: op.clone(),
            key,
            group: ready.write.group().into(),
            kind: ready.write.kind(),
            target: ready.write.ids(),
            payload: ready.write.payload(),
            same: ready.write.same(),
            evidence: ready.evidence,
            state: State::Prepared,
            by: None,
            made: None,
            checked: false,
            answer: None,
            looked: false,
            closed: None,
            beside: Beside::default(),
            prepared_at: book.now(),
            dispatched_at: None,
            ended_at: None,
        };
        match &ready.write {
            Write::Upload { art, .. } => entry.evidence.art = *art,
            Write::CreatePost { of, draft, .. } => (entry.evidence.draft, entry.evidence.revision) = (Some(of.clone()), Some(draft.revision())),
            Write::MemberRole { .. } => entry.target.membership = ready.member.as_ref().and_then(Member::membership).map(|m| m.digest()),
            _ => {}
        }
        if let Some(gone) = book.may_have_gone(&entry) {
            drop_staged(&ready.write);
            return Ok(Reply::MaySent { op: gone });
        }
        let (warning, told) = book.conflicts(&entry);
        let (before, whole) = (entry.evidence.before.clone().unwrap_or_default(), entry.evidence.whole);
        let sealed = Sealed {
            op: op.clone(),
            group: entry.group.clone(),
            group_name: ready.group_name,
            confirm: ready.permit.needs_confirmation() || warning.is_some() || ready.write.back().is_some(),
            write: ready.write,
            role: ready.role,
            post: ready.post,
            member: ready.member,
            art: ready.art,
            labels: ready.labels,
            warning,
            digest: String::new(),
        }
        .seal();
        book.prepare(entry).map_err(not)?;
        let mut rt = self.groups.locked();
        if rt.prepares.get(&link) != Some(&turn) {
            // Its window closed, or asked for another since: over before it was shown.
            rt.ledger.never_sent(&op);
            return Err(Failure::Expired);
        }
        if rt.intents.len() >= INTENTS_MAX {
            let oldest = rt.intents.iter().min_by_key(|i| i.at).map(|i| i.sealed.op.clone()).unwrap_or_default();
            rt.end_unsent(|i| i.sealed.op == oldest);
        }
        rt.intents.push(Intent { scope: scope.clone(), link, sealed: sealed.clone(), picture: ready.picture, offset: ready.offset, file: ready.file, previous: ready.previous, before, whole, told, at: Instant::now() });
        Ok(Reply::Prepared(Box::new(sealed)))
    }

    /// Staged pictures nothing is for any more (a window that ended before its prepare, an earlier program) go (J3).
    fn groups_sweep_staged(&self) {
        let used: Vec<String> = self.groups.locked().intents.iter().filter_map(|i| if let Write::Upload { staged, .. } = &i.sealed.write { Some(staged.name.clone()) } else { None }).collect();
        let Ok(files) = std::fs::read_dir(staging_dir()) else { return };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let old = f.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STAGED_FOR);
            if old && name.ends_with(".png") && !used.contains(&name) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }

    /// The sealed picture's bytes, as sealed.
    fn groups_sealed_art(&self, link: u64, scope: &Scope, op: &str) -> Result<Vec<u8>, Failure> {
        let (write, picture) = {
            let rt = self.groups.locked();
            let i = rt.intents.iter().find(|i| i.sealed.op == op && i.link == link && i.scope == *scope).ok_or(Failure::Expired)?;
            (i.sealed.write.clone(), i.picture.clone())
        };
        match (write, picture) {
            (Write::Upload { staged, .. }, _) => staged.read(),
            (_, Some(bytes)) => Ok(bytes),
            _ => Err(Failure::Expired),
        }
    }

    /// The write checked against what VRChat says now: normalised, allowed (and whether it is confirmed), its target as
    /// read, and what a check will compare with. Reads only, each letting the client's lock go.
    fn groups_validate(&self, scope: &Scope, write: Write) -> Result<Ready, Failure> {
        let me = scope_user(scope)?;
        let me = me.as_str();
        if !g::is_group_id(write.group()) {
            return Err(Failure::Api(GroupFailure::NotFound));
        }
        let names = |group: &Group, catalogue: Option<&[PermissionInfo]>, roles: &[String], rights: &[String]| -> Vec<(String, String)> {
            let mut out: Vec<(String, String)> = Vec::new();
            for id in roles {
                if let Some(r) = group.roles.as_ref().and_then(|l| l.iter().find(|r| r.id == *id)) {
                    out.push((id.clone(), g::label(&r.name, g::ROLE_NAME_MAX)));
                }
            }
            for p in rights {
                if let Some(c) = catalogue.and_then(|l| l.iter().find(|c| c.name == *p)).filter(|c| !c.display_name.is_empty()) {
                    out.push((p.clone(), g::label(&c.display_name, 64)));
                }
            }
            out
        };
        let role_of = |group: &Group, id: &str| group.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == id)).cloned();
        match write {
            Write::MemberRole { group, user, membership, role, add } => {
                let page = self.groups_open(scope, &group)?;
                let permit = authority(&Action::AssignRole { role: &role, user: &user, membership: &membership, add }, &page.group, me, None)?;
                let found = role_of(&page.group, &role).ok_or(Failure::NotRead(Missing::RoleDefinition))?;
                // The member as VRChat has them now, whole: their id, this group, this membership and their role arrays
                // (Blocker 4, verify3 Blocker 1). A lookup of oneself gives no name: the account's own membership counts.
                let member = if user == me {
                    let mine = own_record(&page.group).ok_or(Failure::NotRead(Missing::Member))?;
                    let name = self.view().vrchat_login.user.as_ref().map(|u| g::label(&u.display_name, 32)).filter(|n| !n.is_empty());
                    Member { display_name: name, ..mine }
                } else {
                    self.groups_read(scope, |c| c.group_member(&group, &user))?.ok_or(Failure::Api(GroupFailure::NotFound))?
                };
                if !member.is_of(&group, &user) {
                    return Err(Failure::NotRead(Missing::Member));
                }
                // Of the very membership of the row it was asked on: one who left and came back is another row, pressed
                // again (verify4 F1).
                if member.membership().as_ref() != Some(&membership) {
                    return Err(Failure::Rejoined);
                }
                let (held, member) = (member.role_ids.clone(), Some(member));
                if held.contains(&role) == add {
                    return Err(Failure::Changed);
                }
                let labels = names(&page.group, page.catalogue.as_deref(), &[], &found.permissions);
                Ok(Ready { role: Some(found), member, labels, ..Ready::new(Write::MemberRole { group, user, membership, role, add }, permit, &page.group) })
            }
            Write::CreateRole { group, draft } => {
                let draft = RoleDraft { name: g::label(&draft.name, g::ROLE_NAME_MAX), description: g::text(&draft.description, g::ROLE_DESCRIPTION_MAX), permissions: g::in_order(&draft.permissions), ..draft };
                if draft.name.is_empty() {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                let page = self.groups_open(scope, &group)?;
                let permit = authority(&Action::CreateRole { draft: &draft }, &page.group, me, page.catalogue.as_deref())?;
                let before = page.group.roles.as_ref().map(|r| r.iter().map(|r| known_id(&r.id)).collect());
                let evidence = Evidence { content: Some(draft.as_role().digest_of(&RoleField::ALL)), before, ..Default::default() };
                let labels = names(&page.group, page.catalogue.as_deref(), &[], &draft.permissions);
                Ok(Ready { evidence, labels, ..Ready::new(Write::CreateRole { group, draft }, permit, &page.group) })
            }
            Write::EditRole { group, baseline, edit, back } => {
                // The person's own words are cleaned; those they left as they were are no part of the change.
                let edit = RoleEdit {
                    name: if edit.name == baseline.name { edit.name } else { g::label(&edit.name, g::ROLE_NAME_MAX) },
                    description: if edit.description == baseline.description { edit.description } else { g::text(&edit.description, g::ROLE_DESCRIPTION_MAX) },
                    ..edit
                };
                let page = self.groups_open(scope, &group)?;
                let now = role_of(&page.group, &baseline.id).ok_or(Failure::Api(GroupFailure::NotFound))?;
                if !now.whole {
                    return Err(Failure::NotRead(Missing::RoleDefinition));
                }
                // Made against what is no longer so (another manager changed it): nothing is sent (M4).
                same_role(&baseline, &now)?;
                let change = RoleChange::between(&now, &edit);
                if change.is_empty() {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                let permit = authority(&Action::EditRole { role: &now.id, change: &change }, &page.group, me, None)?;
                let fields = change.fields();
                let evidence = Evidence { wanted: Some(now.with(&change).digest_of(&fields)), role_fields: fields, was: now.said(), ..Default::default() };
                let labels = names(&page.group, page.catalogue.as_deref(), &[], &now.permissions);
                Ok(Ready { role: Some(now.clone()), evidence, labels, ..Ready::new(Write::EditRole { group, baseline: now, edit, back }, permit, &page.group) })
            }
            Write::DeleteRole { group, role } => {
                let page = self.groups_open(scope, &group)?;
                let permit = authority(&Action::DeleteRole { role: &role }, &page.group, me, None)?;
                let found = role_of(&page.group, &role).ok_or(Failure::Api(GroupFailure::NotFound))?;
                let labels = names(&page.group, page.catalogue.as_deref(), &[], &found.permissions);
                let before = page.group.roles.as_ref().map(|r| r.iter().map(|r| known_id(&r.id)).collect());
                let evidence = Evidence { before, whole: true, ..Default::default() };
                Ok(Ready { role: Some(found), labels, evidence, ..Ready::new(Write::DeleteRole { group, role }, permit, &page.group) })
            }
            Write::CreatePost { group, draft, of } => {
                let draft = clean_post(draft)?;
                if of.len() > 64 {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                let found = self.groups_read(scope, |c| c.group(&group))?;
                let permit = authority(&Action::Post, &found, me, None)?;
                let posts = self.groups_read(scope, |c| c.group_posts(&group, 0))?;
                let evidence = Evidence { content: Some(draft.content()), before: Some(posts.items.iter().map(|p| known_id(&p.id)).collect()), ..Default::default() };
                let labels = names(&found, None, &draft.role_ids, &[]);
                Ok(Ready { evidence, labels, ..Ready::new(Write::CreatePost { group, draft, of }, permit, &found) })
            }
            Write::DeletePost { group, post } => {
                if !g::is_post_id(&post.id) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                // One whose audience or picture was not read whole is not deleted from here (verify2 B7).
                if !post.complete {
                    return Err(Failure::NotRead(Missing::Post));
                }
                let found = self.groups_read(scope, |c| c.group(&group))?;
                let permit = authority(&Action::Post, &found, me, None)?;
                // Deleted only when the post found now is the one shown (Blocker 7); one not found is not deleted blind.
                let (now, offset) = self.groups_find_post(scope, &group, &post.id)?.ok_or(Failure::NotRead(Missing::Post))?;
                if !now.complete {
                    return Err(Failure::NotRead(Missing::Post));
                }
                same_post(&post, &now)?;
                let labels = names(&found, None, &now.role_ids, &[]);
                Ok(Ready { post: Some(now.clone()), labels, offset, ..Ready::new(Write::DeletePost { group, post: now }, permit, &found) })
            }
            Write::Upload { tag, group, staged, art } => {
                // The file as announced (Major 1), then the account's right to upload, apart from the group's (Major 4).
                let bytes = staged.read()?;
                let value = self.groups_read(scope, |c| c.current_user_value().map_err(|_| GroupFailure::Unavailable))?;
                let permit = permit(g::can_upload(me, &group, own_tags(&value, me).as_deref()))?;
                let found = self.groups_read(scope, |c| c.group(&group))?;
                let (_, ids, whole) = self.groups_inventory(scope, tag)?;
                let evidence = Evidence {
                    content: Some(staged.sha256.clone()),
                    before: Some(ids.iter().map(|id| known_id(id)).collect()),
                    whole,
                    md5: Some(orbiters_core::vrchat_api::group_file_md5(&bytes)),
                    size: Some(bytes.len() as u64),
                    ..Default::default()
                };
                let sealed = SealedArt { file: None, sha256: staged.sha256.clone(), size: staged.size, version: None };
                Ok(Ready { evidence, art: Some(sealed), ..Ready::new(Write::Upload { tag, group, staged, art }, permit, &found) })
            }
            Write::ApplyArt { group, slot, file, back } => {
                if !g::is_file_id(&file) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let found = self.groups_read(scope, |c| c.group(&group))?;
                let permit = authority(&Action::ApplyArt, &found, me, None)?;
                // What the group has now must be known: what a check, and the last read, compare with.
                let previous = found.art(slot).ok_or(Failure::NotRead(Missing::Picture))?.map(str::to_string);
                let evidence = Evidence { was: found.said.clone(), ..Default::default() };
                if let Some(op) = back {
                    // What was there just before the write `op`, as the service itself read it then, and no other file.
                    let key = self.groups_key(scope)?;
                    let was = self.groups.locked().was.iter().find(|w| w.op == op && w.key == key).cloned().ok_or(Failure::Expired)?;
                    let Before::Group(then) = was.what else { return Err(Failure::Expired) };
                    let url = match slot {
                        ArtSlot::Banner => then.banner_url.clone(),
                        ArtSlot::Icon => then.icon_url.clone(),
                    };
                    if then.id != group || then.art(slot) != Some(Some(file.as_str())) {
                        return Err(Failure::Expired);
                    }
                    if previous.as_deref() == Some(file.as_str()) {
                        return Err(Failure::Changed);
                    }
                    let bytes = self.groups_read(scope, |c| c.image(&url).map_err(|_| GroupFailure::Unavailable))?;
                    if bytes.is_empty() || bytes.len() > PICTURE_MAX {
                        return Err(Failure::NotRead(Missing::Picture));
                    }
                    let sealed = SealedArt { file: Some(file.clone()), sha256: sha256(&bytes), size: bytes.len() as u64, version: None };
                    return Ok(Ready { art: Some(sealed), picture: Some(bytes), previous, evidence, ..Ready::new(Write::ApplyArt { group, slot, file, back: Some(op) }, permit, &found) });
                }
                // One of the account's own, of the kind the slot takes, said by VRChat of that very file; the version it is
                // now (verify3 Major 4).
                let filed = self.groups_file(scope, slot.tag(), &file)?;
                own_picture(&filed.detail, slot.tag(), me)?;
                let version = filed.detail.version.ok_or(Failure::NotRead(Missing::Picture))?;
                // The picture that file is, as VRChat serves it: its bytes are what the confirmation shows (B-E).
                let url = filed.detail.file.url.clone();
                let bytes = self.groups_read(scope, |c| c.image(&url).map_err(|_| GroupFailure::Unavailable))?;
                if bytes.is_empty() || bytes.len() > PICTURE_MAX {
                    return Err(Failure::NotRead(Missing::Picture));
                }
                let sealed = SealedArt { file: Some(file.clone()), sha256: sha256(&bytes), size: bytes.len() as u64, version: Some(version) };
                Ok(Ready { art: Some(sealed), picture: Some(bytes), file: Some(filed), previous, evidence, ..Ready::new(Write::ApplyArt { group, slot, file, back: None }, permit, &found) })
            }
        }
    }

    /// The sealed write `op`, once, as it was shown (`digest`).
    fn groups_commit(&self, link: u64, scope: &Scope, op: &str, digest: &str) -> (Outcome, Option<GroupRefusal>) {
        let book = self.groups_book();
        let intent = {
            let mut rt = self.groups.locked();
            let Some(at) = rt.intents.iter().position(|i| i.sealed.op == op) else {
                return (Outcome::Refused(if rt.used.iter().any(|u| u == op) { Failure::Duplicate } else { Failure::Expired }), None);
            };
            // Another window's (or the overlay's) sealed write is not this one's to send.
            if rt.intents[at].link != link || rt.intents[at].scope != *scope {
                return (Outcome::Refused(Failure::Expired), None);
            }
            // What was shown is not what is sealed: nothing goes, and it is over.
            if rt.intents[at].sealed.digest() != digest {
                rt.end_unsent(|i| i.sealed.op == op);
                return (Outcome::Refused(Failure::Changed), None);
            }
            let target = rt.intents[at].sealed.write.target();
            if rt.writing.contains(&target) {
                return (Outcome::Refused(Failure::Busy), None);
            }
            let intent = rt.intents.remove(at);
            rt.used.push_back(op.to_string());
            while rt.used.len() > USED_MAX {
                rt.used.pop_front();
            }
            rt.writing.insert(target);
            intent
        };
        /// Whatever happens on the way (a panic included), the target is let go and what went out is not called unsent.
        struct Going<'a>(&'a Controller, &'a Ledger, String, String, bool);
        impl Drop for Going<'_> {
            fn drop(&mut self) {
                self.0.groups.locked().writing.remove(&self.3);
                if !self.4 {
                    self.1.end(&self.2, State::Unknown, None, None, Beside::default());
                }
            }
        }
        let mut going = Going(self, &book, op.to_string(), intent.sealed.write.target(), false);
        let (gone, beside, said) = self.groups_dispatch(scope, &intent);
        let (outcome, state, by, made) = match gone {
            Gone::Stopped(f) => (Outcome::Refused(f), State::Refused, Some(By::BeforeSending), None),
            Gone::Sent(Ok(done)) => {
                let made = done.made();
                (Outcome::Done(done), State::Done, Some(By::Answer), made)
            }
            Gone::Sent(Err(GroupFailure::Unknown)) => (Outcome::Unknown, State::Unknown, None, None),
            // Ours, before the request went; or VRChat's own "no".
            Gone::Sent(Err(GroupFailure::Disabled)) => (Outcome::Refused(Failure::AccountChanged), State::Refused, Some(By::BeforeSending), None),
            Gone::Sent(Err(e @ (GroupFailure::RateLimited(_) | GroupFailure::SignedOut))) => (Outcome::Refused(Failure::Api(e)), State::Refused, Some(By::BeforeSending), None),
            Gone::Sent(Err(e)) => (Outcome::Refused(Failure::Api(e)), State::Refused, Some(By::Answer), None),
        };
        // Kept to put back what moved beside it, as long as something did or may have.
        if !beside.any() && state != State::Unknown {
            self.groups.locked().was.retain(|w| w.op != op);
        }
        if let (State::Done, Some(put_right), Ok(key)) = (state, intent.sealed.write.back(), self.groups_key(scope)) {
            book.put_back(&key, put_right, &intent.sealed.write.parts());
        }
        book.end(op, state, by, made, beside);
        going.4 = true;
        drop_staged(&intent.sealed.write);
        // VRChat's own words, only with VRChat's own "no".
        let said = said.filter(|_| matches!(outcome, Outcome::Refused(Failure::Api(GroupFailure::Forbidden | GroupFailure::Invalid | GroupFailure::NotFound))));
        (outcome, said)
    }

    /// Under the client's lock, for this very account: see [`Controller::groups_step`].
    fn groups_dispatch(&self, scope: &Scope, intent: &Intent) -> (Gone, Beside, Option<GroupRefusal>) {
        let Ok(user) = scope_user(scope) else { return (Gone::Stopped(Failure::SignedOut), Beside::default(), None) };
        let source = self.vrchat_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut beside = Beside::default();
        let mut said = None;
        // The scope is looked at inside, before the request and not after: what was sent is never called "not sent".
        let sent = source.personal_until(&user, || true, deadline, |c| {
            if !self.groups_live(scope) || self.view().vrchat_login.presence() != Presence::SignedIn {
                return Ok(Gone::Stopped(Failure::AccountChanged));
            }
            c.take_group_refusal();
            let step = self.groups_step(c, scope, &user, intent, &mut beside);
            said = c.take_group_refusal();
            Ok(match step {
                Ok(sent) => Gone::Sent(sent),
                Err(f) => Gone::Stopped(f),
            })
        });
        let gone = match sent {
            Ok(gone) => gone,
            // Refused before anything went: the session or the grant.
            Err(orbiters_core::vrchat_api::Error::Disabled) => Gone::Stopped(Failure::AccountChanged),
            Err(_) => Gone::Stopped(Failure::SignedOut),
        };
        (gone, beside, said)
    }

    /// The send, in this order: the reads that decide it and are not the last; the ledger marked `Dispatched` on the
    /// disk; the write's pacing waited for; then, with nothing else between, the LAST read and the request, whose leave is
    /// given (or not) as it is handed to the network ([`Controller::groups_leave`]). What is left open is the time between
    /// VRChat's answer to that read and its taking the request: no request can close it, and the confirmations say so.
    /// `Err`: nothing went.
    fn groups_step(&self, c: &mut Client, scope: &Scope, me: &str, intent: &Intent, beside: &mut Beside) -> Result<Result<Done, GroupFailure>, Failure> {
        let (sealed, write) = (&intent.sealed, &intent.sealed.write);
        let mut last = Last::default();
        // What is read first.
        match write {
            Write::CreateRole { group, draft } if !draft.permissions.is_empty() => last.catalogue = read(c.group_permission_catalogue(group))?,
            Write::DeletePost { group, .. } => last.group = Some(read(c.group(group))?),
            Write::MemberRole { group, user, .. } if user != me => last.group = Some(read(c.group(group))?),
            Write::Upload { staged, .. } => last.png = Some(staged.read()?),
            _ => {}
        }
        // What those refuse is not called sent, not even on the disk.
        if last.group.is_some() {
            self.groups_allowed(me, intent, &last)?;
        }
        if !self.groups_live(scope) {
            return Err(Failure::AccountChanged);
        }
        match self.groups_book().dispatch(&sealed.op, &intent.told) {
            Ok(()) => {}
            Err(Stop::Not(n)) => return Err(not(n)),
            Err(Stop::MaySent) => return Err(Failure::MaySent),
            Err(Stop::Since) => return Err(Failure::Since),
            Err(Stop::NotPrepared) => return Err(Failure::Expired),
        }
        read(c.ready_to_write())?;
        // The last read.
        match write {
            Write::MemberRole { group, user, .. } if user != me => last.member = Some(read(c.group_member(group, user))?.ok_or(Failure::Api(GroupFailure::NotFound))?),
            Write::DeletePost { group, .. } => last.posts = Some(read(c.group_posts(group, intent.offset))?.items),
            Write::ApplyArt { group, slot, file, back } => {
                if back.is_none() {
                    // The very version that was shown (verify3 Major 4); what VRChat takes is the file, so the instant
                    // between is said in the confirmation.
                    let filed = intent.file.as_ref().ok_or(Failure::Expired)?;
                    last.file = Some(match filed.page {
                        None => read(c.picture_file(file))?,
                        Some(offset) => read(c.inventory(slot.tag(), offset))?.details.into_iter().find(|d| d.file.id == *file).ok_or(Failure::Changed)?,
                    });
                }
                last.group = Some(read(c.group(group))?);
            }
            Write::Upload { .. } => last.value = Some(read(c.current_user_value().map_err(|_| GroupFailure::Unavailable))?),
            Write::MemberRole { group, .. } | Write::CreateRole { group, .. } | Write::EditRole { group, .. } | Write::DeleteRole { group, .. } | Write::CreatePost { group, .. } => last.group = Some(read(c.group(group))?),
        }
        self.groups_remember(scope, intent, &last);
        self.groups_send(c, scope, me, intent, &last, beside)
    }

    /// What the group or the role is just before the request, in memory, to put back what moves beside it; as hashes in the
    /// record, for a check.
    fn groups_remember(&self, scope: &Scope, intent: &Intent, last: &Last) {
        let op = &intent.sealed.op;
        let (Some(group), Ok(key)) = (&last.group, self.groups_key(scope)) else { return };
        let (what, said) = match &intent.sealed.write {
            Write::ApplyArt { .. } => (Before::Group(Box::new(group.clone())), group.said.clone()),
            Write::EditRole { baseline, .. } => match group.roles.as_ref().and_then(|l| l.iter().find(|r| r.id == baseline.id)) {
                Some(r) => (Before::Role { before: r.clone(), after: None }, r.said()),
                None => return,
            },
            _ => return,
        };
        self.groups_book().was(op, &said);
        let mut rt = self.groups.locked();
        rt.was.push_back(Was { op: op.clone(), key, what });
        while rt.was.len() > WAS_MAX {
            rt.was.pop_front();
        }
    }

    /// Whether the rules, on what was just read, allow exactly what was sealed, on exactly what it was sealed for: the one
    /// place where that is decided at the send.
    fn groups_allowed(&self, me: &str, intent: &Intent, last: &Last) -> Result<Permit, Failure> {
        let (sealed, write) = (&intent.sealed, &intent.sealed.write);
        let group = || last.group.as_ref().ok_or(Failure::NotRead(Missing::Membership));
        match write {
            Write::MemberRole { group: id, user, membership, role, add } => {
                let found = group()?;
                let permit = authority(&Action::AssignRole { role, user, membership, add: *add }, found, me, None)?;
                self.groups_same_role(found, role, sealed)?;
                let now = if user == me { own_record(found) } else { last.member.clone() };
                let Some(now) = now else {
                    // Not read yet (the rules alone, before it is marked as sent).
                    return if user == me { Err(Failure::NotRead(Missing::Member)) } else { Ok(permit) };
                };
                if !now.is_of(id, user) {
                    return Err(Failure::NotRead(Missing::Member));
                }
                // The very membership it was asked on and decided for (verify3 Blocker 1, verify4 F1).
                if now.membership().as_ref() != Some(membership) || sealed.member.as_ref().and_then(Member::membership).as_ref() != Some(membership) {
                    return Err(Failure::Rejoined);
                }
                if now.role_ids.contains(role) == *add {
                    return Err(Failure::Changed);
                }
                Ok(permit)
            }
            Write::CreateRole { draft, .. } => authority(&Action::CreateRole { draft }, group()?, me, last.catalogue.as_deref()),
            Write::EditRole { baseline, edit, .. } => {
                let change = RoleChange::between(baseline, edit);
                let permit = authority(&Action::EditRole { role: &baseline.id, change: &change }, group()?, me, None)?;
                self.groups_same_role(group()?, &baseline.id, sealed)?;
                Ok(permit)
            }
            Write::DeleteRole { role, .. } => {
                let permit = authority(&Action::DeleteRole { role }, group()?, me, None)?;
                self.groups_same_role(group()?, role, sealed)?;
                Ok(permit)
            }
            Write::CreatePost { .. } => authority(&Action::Post, group()?, me, None),
            Write::DeletePost { post: baseline, .. } => {
                let permit = authority(&Action::Post, group()?, me, None)?;
                if let Some(posts) = &last.posts {
                    // Not where it was, not whole, or not as the person saw it (its audience above all): nothing is sent.
                    let now = posts.iter().find(|p| p.id == baseline.id).ok_or(Failure::Changed)?;
                    if !now.complete {
                        return Err(Failure::NotRead(Missing::Post));
                    }
                    same_post(baseline, now)?;
                }
                Ok(permit)
            }
            Write::ApplyArt { slot, back, .. } => {
                let permit = authority(&Action::ApplyArt, group()?, me, None)?;
                if back.is_none() {
                    let (now, filed) = (last.file.as_ref().ok_or(Failure::Expired)?, intent.file.as_ref().ok_or(Failure::Expired)?);
                    own_picture(now, slot.tag(), me)?;
                    if *now != filed.detail {
                        return Err(Failure::Changed);
                    }
                }
                if group()?.art(*slot).ok_or(Failure::NotRead(Missing::Picture))? != intent.previous.as_deref() {
                    return Err(Failure::Changed);
                }
                Ok(permit)
            }
            Write::Upload { group, .. } => {
                let value = last.value.as_ref().ok_or(Failure::NotRead(Missing::Entitlement))?;
                self::permit(g::can_upload(me, group, own_tags(value, me).as_deref()))
            }
        }
    }

    /// Leave for the sealed write to go, given as its request is handed to the network and held until it is answered: the
    /// one place that gives it (verify4 F1, F13). `Err`: it does not go.
    fn groups_leave(&self, scope: &Scope, me: &str, intent: &Intent, last: &Last) -> Result<Box<dyn Held + '_>, Failure> {
        // Whoever the service has installed by now, whatever it has published: an operation of one epoch ends with it.
        let epoch = scope.epoch().filter(|e| *e == self.identity_epoch()).ok_or(Failure::AccountChanged)?;
        // Held from here on: no account is installed before this is answered (N4N0's fence, `pause_effects`).
        let acting = self.n4n0.acting(epoch).ok_or(Failure::AccountChanged)?;
        if !self.vrchat_now().is_some_and(|now| scope.verified_by(&now)) || !self.groups_live(scope) || intent.scope != *scope {
            return Err(Failure::AccountChanged);
        }
        // The confirmation it was given for: this one, on its way, not ended, not old, not used.
        if intent.at.elapsed() >= INTENT_FOR || !self.groups_book().going(&intent.sealed.op) {
            return Err(Failure::Expired);
        }
        let permit = self.groups_allowed(me, intent, last)?;
        let write = &intent.sealed.write;
        if !permit.covers(me, &intent.sealed.group, &write.subject()) || write.group() != intent.sealed.group {
            return Err(Failure::NotRead(Missing::Identity));
        }
        #[cfg(test)]
        {
            self.groups.lock().unwrap().sent_ops += 1;
        }
        Ok(Box::new(acting))
    }

    /// The role as sealed is the role as read now.
    fn groups_same_role(&self, found: &Group, id: &str, sealed: &Sealed) -> Result<(), Failure> {
        let now = found.roles.as_ref().ok_or(Failure::NotRead(Missing::Roles))?.iter().find(|r| r.id == id).ok_or(Failure::Changed)?;
        same_role(sealed.role.as_ref().ok_or(Failure::Changed)?, now)
    }

    /// The request itself, with its leave: there is no other way to VRChat's group writes in this program. `Err`: it did not
    /// go.
    fn groups_send(&self, c: &mut Client, scope: &Scope, me: &str, intent: &Intent, last: &Last, beside: &mut Beside) -> Result<Result<Done, GroupFailure>, Failure> {
        let (sealed, write) = (&intent.sealed, &intent.sealed.write);
        let refused: std::cell::Cell<Option<Failure>> = Default::default();
        let leave: Leave = Box::new(|| self.groups_leave(scope, me, intent, last).map_err(|f| refused.set(Some(f))).ok());
        let sent = match write {
            Write::MemberRole { group, user, role, add, .. } => c.set_member_role(group, user, role, *add, leave).map(|role_ids| Done::MemberRoles { user: user.clone(), role_ids }),
            Write::CreateRole { group, draft } => c.create_role(group, draft, leave).map(Done::Role),
            Write::EditRole { group, baseline, edit, .. } => {
                let change = RoleChange::between(baseline, edit);
                c.update_role(group, &baseline.id, &change, leave).map(|roles| {
                    // The role as just read, with the change: whatever else of it VRChat's answer has otherwise is said.
                    if let (Some(then), Some(now)) = (sealed.role.as_ref(), roles.iter().find(|r| r.id == baseline.id)) {
                        *beside = self.groups_beside_role(&sealed.op, &then.said(), &change, now);
                    }
                    Done::Roles(roles)
                })
            }
            Write::DeleteRole { group, role } => c.delete_role(group, role, leave).and_then(|roles| {
                // Every other one that was there: a list without them is no proof of it (verify4 F3).
                let others = intent.before.len().saturating_sub(1);
                if g::whole_list(&roles, others, |id| intent.before.contains(&known_id(id))) { Ok(Done::Roles(roles)) } else { Err(GroupFailure::Unknown) }
            }),
            Write::CreatePost { group, draft, .. } => c.create_post(group, draft, leave).map(Done::Post),
            Write::DeletePost { group, post } => c.delete_post(group, &post.id, leave).map(|_| Done::PostDeleted { post: post.id.clone() }),
            Write::ApplyArt { group, slot, file, .. } => c.set_group_art(group, *slot, file, leave).map(|mut now| {
                if let Some(then) = &last.group {
                    *beside = then.moved_beside(*slot, &now);
                    // What its answer left out is read, so that "not said" is not taken for "the same"; what either shows moved
                    // is said (verify4 F7).
                    if !beside.unread.is_empty() {
                        if let Ok(read) = c.group(group) {
                            let again = then.moved_beside(*slot, &read);
                            beside.moved.extend(again.moved.into_iter().filter(|p| !beside.moved.contains(p)).collect::<Vec<_>>());
                            beside.moved.sort();
                            beside.unread = again.unread.into_iter().filter(|p| beside.unread.contains(p)).collect();
                            if read.art(*slot) == Some(Some(file.as_str())) {
                                now = read;
                            }
                        }
                    }
                }
                Done::Group(Box::new(now))
            }),
            Write::Upload { tag, .. } => match last.png.clone() {
                // VRChat's own new file (it may keep other bytes than those it was given): not one that was there, by a list
                // of all that were. Without such a list no id is known to be new, whatever time it says (verify4 F2).
                Some(png) => c.upload_image(*tag, png, leave).and_then(|made| if !intent.whole || intent.before.contains(&known_id(&made.file.id)) { Err(GroupFailure::Unknown) } else { Ok(Done::Uploaded(made.file)) }),
                None => return Err(Failure::Expired),
            },
        };
        match refused.take() {
            Some(f) => Err(f),
            None => Ok(sent),
        }
    }

    /// What of the role is otherwise than it was, beside `change`; the role as it is now, kept to put that back.
    fn groups_beside_role(&self, op: &str, then: &g::Said, change: &RoleChange, now: &Role) -> Beside {
        if let Some(Was { what: Before::Role { after, .. }, .. }) = self.groups.locked().was.iter_mut().find(|w| w.op == op) {
            *after = Some(now.clone());
        }
        let asked: Vec<Part> = change.fields().into_iter().map(RoleField::part).collect();
        Beside::between(then, &now.said(), &asked)
    }

    /// What VRChat shows now of `op`: never what happened, or who did it (verify3 Blocker 2).
    fn groups_check(&self, link: u64, scope: &Scope, op: &str) -> Result<Observation, Failure> {
        let key = self.groups_key(scope)?;
        let book = self.groups_book();
        if !book.owned() {
            return Err(Failure::OtherOwner);
        }
        // Sealed and never committed: it ends here, unsent.
        self.groups.locked().end_unsent(|i| i.sealed.op == op && i.link == link && i.scope == *scope);
        // Not there, or not readable: that says nothing of what was sent (verify3 Major 1).
        let Some(entry) = book.get(&key, op) else { return Ok(Observation::CannotTell(Unclear::NoRecord)) };
        match entry.state {
            State::Done => return Ok(Observation::Answered { done: true }),
            State::Refused => return Ok(Observation::Answered { done: false }),
            State::Prepared => return Ok(Observation::Running),
            _ if book.active(op) => return Ok(Observation::Running),
            // Settled before: what VRChat shows now was not read, and is not said (verify4 F4).
            _ if entry.closed.is_some() => return Ok(Observation::Closed),
            State::Dispatched | State::Unknown => {}
        }
        book.looking(op);
        let mut beside = None;
        let seen = match self.groups_reconcile(scope, &entry, &mut beside) {
            Ok(seen) => seen,
            Err(f @ (Failure::AccountChanged | Failure::SignedOut)) => return Err(f),
            // What cannot be read is no dead end: the person may still decide (verify3 Major 3).
            Err(f) => Seen::CannotTell(Unclear::Failed(f)),
        };
        self.groups.locked().candidates.remove(op);
        Ok(match seen {
            Seen::Shows => {
                if !book.close(op, Closing::Shows, false, None) {
                    return Err(Failure::LookNotKept);
                }
                if let Some(beside) = beside.filter(Beside::any) {
                    book.beside(op, beside);
                } else {
                    self.groups.locked().was.retain(|w| w.op != op);
                }
                Observation::Shows
            }
            // Read too soon after the send, its absence says nothing.
            Seen::ShowsNot if book.now() - entry.dispatched_at.unwrap_or(0) < SETTLES_AFTER_MS => {
                book.answer(op, Answer::CannotTell, false);
                Observation::CannotTell(Unclear::TooSoon)
            }
            Seen::ShowsNot => {
                book.answer(op, Answer::ShowsNot, false);
                Observation::ShowsNot
            }
            Seen::Candidates(list) => {
                book.answer(op, Answer::CannotTell, false);
                self.groups.locked().candidates.insert(op.to_string(), list.iter().map(|c| c.id.clone()).collect());
                Observation::Candidates(list)
            }
            Seen::CannotTell(why) => {
                book.answer(op, Answer::CannotTell, false);
                Observation::CannotTell(why)
            }
        })
    }

    /// What the person decides of a write no one can tell the end of, kept as theirs. One of those shown is only what they
    /// were shown.
    fn groups_decide(&self, scope: &Scope, op: &str, choice: Choice) -> Result<(), Failure> {
        let key = self.groups_key(scope)?;
        let book = self.groups_book();
        book.get(&key, op).ok_or(if book.owned() { Failure::Expired } else { Failure::OtherOwner })?;
        let kept = match choice {
            Choice::Shows { made } => {
                let offered = self.groups.locked().candidates.get(op).cloned().unwrap_or_default();
                if made.as_ref().is_some_and(|m| !offered.contains(m)) {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                book.close(op, Closing::Shows, true, made)
            }
            Choice::ShowsNot => book.answer(op, Answer::ShowsNot, true),
            Choice::Again => book.close(op, Closing::Again, true, None),
            Choice::Drop => book.close(op, Closing::Dropped, true, None),
        };
        if kept && book.get(&key, op).is_some_and(|e| !e.open()) {
            self.groups.locked().candidates.remove(op);
        }
        if kept { Ok(()) } else { Err(Failure::LookNotKept) }
    }

    /// A fresh read of the very object a write touched, per kind: whether VRChat shows now what was asked. Anything
    /// missing, or of another object or membership, tells nothing. What has no id until VRChat answers is for the person.
    fn groups_reconcile(&self, scope: &Scope, e: &Entry, beside: &mut Option<Beside>) -> Result<Seen, Failure> {
        let me = scope_user(scope)?;
        let group = e.group.as_str();
        let there_before = |id: &str| e.evidence.before.as_ref().is_some_and(|b| b.contains(&known_id(id)));
        let shows = |yes: bool| if yes { Seen::Shows } else { Seen::ShowsNot };
        let like = |same: bool| if same { Likeness::Exactly } else { Likeness::NotExactly };
        Ok(match e.kind {
            Kind::AssignRole | Kind::UnassignRole => {
                let (Some(user), Some(role)) = (&e.target.user, &e.target.role) else { return Ok(Seen::CannotTell(Unclear::NotRead)) };
                let now = if *user == me {
                    own_record(&self.groups_read(scope, |c| c.group(group))?)
                } else {
                    self.groups_read(scope, |c| c.group_member(group, user))?
                };
                match now {
                    None => Seen::CannotTell(Unclear::NotSeen),
                    Some(m) if !m.is_of(group, user) => Seen::CannotTell(Unclear::NotRead),
                    // Another membership of the same person: what it holds says nothing of the one it was for.
                    Some(m) if m.membership().map(|m| m.digest()) != e.target.membership => Seen::CannotTell(Unclear::Rejoined),
                    Some(m) => shows(m.role_ids.contains(role) == (e.kind == Kind::AssignRole)),
                }
            }
            Kind::CreateRole => {
                let Some(roles) = self.groups_read(scope, |c| c.group(group))?.roles else { return Ok(Seen::CannotTell(Unclear::NotRead)) };
                let same = |r: &Role| r.whole && Some(r.digest_of(&RoleField::ALL)) == e.evidence.content;
                Seen::Candidates(roles.iter().filter(|r| !there_before(&r.id)).map(|r| Candidate { id: r.id.clone(), label: g::label(&r.name, g::ROLE_NAME_MAX), url: String::new(), at: r.created_at.clone(), like: like(same(r)) }).take(CANDIDATES_MAX).collect())
            }
            Kind::EditRole => {
                let Some(roles) = self.groups_read(scope, |c| c.group(group))?.roles else { return Ok(Seen::CannotTell(Unclear::NotRead)) };
                match roles.iter().find(|r| Some(&r.id) == e.target.role.as_ref()) {
                    Some(r) if !r.whole => Seen::CannotTell(Unclear::NotRead),
                    Some(r) => {
                        let asked: Vec<Part> = e.evidence.role_fields.iter().map(|f| f.part()).collect();
                        *beside = Some(Beside::between(&e.evidence.was, &r.said(), &asked));
                        if let Some(Was { what: Before::Role { after, .. }, .. }) = self.groups.locked().was.iter_mut().find(|w| w.op == e.op) {
                            *after = Some(r.clone());
                        }
                        shows(Some(r.digest_of(&e.evidence.role_fields)) == e.evidence.wanted)
                    }
                    None => Seen::CannotTell(Unclear::NotSeen),
                }
            }
            Kind::DeleteRole => match self.groups_read(scope, |c| c.group(group))?.roles {
                None => Seen::CannotTell(Unclear::NotRead),
                // Still there says it plainly; gone says it only of the whole list: those no group is without, and every
                // other one there was (verify4 F3).
                Some(roles) if roles.iter().any(|r| Some(&r.id) == e.target.role.as_ref()) => Seen::ShowsNot,
                Some(roles) => match &e.evidence.before {
                    Some(before) if g::whole_list(&roles, before.len().saturating_sub(1), |id| before.contains(&known_id(id))) => Seen::Shows,
                    _ => Seen::CannotTell(Unclear::NotRead),
                },
            },
            Kind::CreatePost => {
                let early = e.prepared_at - CLOCKS_DIFFER_MS;
                let made_by_then = |p: &Post| p.created_at.as_deref().and_then(millis).is_none_or(|at| at >= early);
                let mut found = Vec::new();
                let mut offset = 0;
                for _ in 0..POST_PAGES {
                    let page = self.groups_read(scope, |c| c.group_posts(group, offset))?;
                    offset += page.raw;
                    let older = page.items.iter().any(|p| !made_by_then(p));
                    found.extend(page.items);
                    if older || page.raw < orbiters_core::vrchat_api::GROUP_PAGE {
                        break;
                    }
                }
                let same = |p: &Post| p.complete && Some(p.content()) == e.evidence.content;
                Seen::Candidates(
                    found
                        .iter()
                        .filter(|p| p.author_id == me && made_by_then(p) && !there_before(&p.id))
                        .map(|p| Candidate { id: p.id.clone(), label: g::label(&p.title, g::POST_TITLE_MAX), url: String::new(), at: p.created_at.clone(), like: like(same(p)) })
                        .take(CANDIDATES_MAX)
                        .collect(),
                )
            }
            Kind::DeletePost => {
                let Some(id) = &e.target.post else { return Ok(Seen::CannotTell(Unclear::NotRead)) };
                match self.groups_find_post(scope, group, id)? {
                    Some(_) => Seen::ShowsNot,
                    // VRChat has no call for one post by its id, and its list is filtered by who may see it: a post not
                    // in it is deleted, or hidden, or out of reach. The person looks.
                    None => Seen::CannotTell(Unclear::NotSeen),
                }
            }
            Kind::ApplyBanner | Kind::ApplyIcon => {
                let found = self.groups_read(scope, |c| c.group(group))?;
                if let Some(slot) = e.kind.slot() {
                    *beside = Some(Beside::between(&e.evidence.was, &found.said, &[slot.part()]));
                }
                match e.kind.slot().and_then(|s| found.art(s)) {
                    Some(now) => shows(now.is_some() && now == e.target.file.as_deref()),
                    None => Seen::CannotTell(Unclear::NotRead),
                }
            }
            Kind::UploadGallery | Kind::UploadIcon => {
                let Some(tag) = e.kind.tag() else { return Ok(Seen::CannotTell(Unclear::NotRead)) };
                // The account's own of that kind that were not there before, by their ids. Where all there was is not known, by
                // the time they say too (clocks differ: it proves nothing, and one with no time is offered all the same).
                // Never by their bytes: VRChat may keep others; the very ones are a bonus (verify4 F2).
                let whole = e.evidence.whole;
                let exact = |f: &GroupFile| f.md5.is_some() && f.md5 == e.evidence.md5 && f.size.is_some() && f.size == e.evidence.size;
                let kind = |f: &GroupFile| f.tags.as_ref().map(|t| t.iter().any(|t| t == tag.wire()));
                let like = |f: &GroupFile| match kind(f) {
                    Some(true) if exact(f) => Likeness::Exactly,
                    Some(true) if whole => Likeness::Since,
                    _ => Likeness::Uncertain,
                };
                let early = e.dispatched_at.unwrap_or(e.prepared_at) - CLOCKS_DIFFER_MS;
                let may = |f: &GroupFile| whole || f.created_at.as_deref().and_then(millis).is_none_or(|at| at >= early);
                let (files, _, _) = self.groups_inventory(scope, tag)?;
                Seen::Candidates(
                    files
                        .iter()
                        .filter(|f| f.owner == me && kind(f) != Some(false) && may(f) && !there_before(&f.file.id))
                        .map(|f| Candidate { id: f.file.id.clone(), label: String::new(), url: f.file.url.clone(), at: f.created_at.clone(), like: like(f) })
                        .take(CANDIDATES_MAX)
                        .collect(),
                )
            }
        })
    }
}

/// What was read for a send, the last read included: what its leave is decided on.
#[derive(Default)]
struct Last {
    group: Option<Group>,
    member: Option<Member>,
    posts: Option<Vec<Post>>,
    file: Option<GroupFile>,
    value: Option<serde_json::Value>,
    catalogue: Option<Vec<PermissionInfo>>,
    png: Option<Vec<u8>>,
}

/// How a dispatch went: stopped before the request, or sent and answered.
#[allow(clippy::large_enum_variant)]
enum Gone {
    Stopped(Failure),
    Sent(Result<Done, GroupFailure>),
}

/// What VRChat shows now of the very object.
enum Seen {
    Shows,
    ShowsNot,
    Candidates(Vec<Candidate>),
    CannotTell(Unclear),
}

/// Milliseconds since 1970 of a time VRChat gave.
fn millis(at: &str) -> Option<i64> {
    let t = time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?;
    Some((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

impl Done {
    /// The id of what it made, for the ledger.
    fn made(&self) -> Option<String> {
        match self {
            Done::Role(r) => Some(r.id.clone()),
            Done::Post(p) => Some(p.id.clone()),
            Done::Uploaded(f) => Some(f.id.clone()),
            _ => None,
        }
    }
}

/// What a write needs once checked.
struct Ready {
    write: Write,
    permit: Permit,
    group_name: String,
    role: Option<Role>,
    post: Option<Post>,
    member: Option<Member>,
    art: Option<SealedArt>,
    picture: Option<Vec<u8>>,
    offset: usize,
    file: Option<Filed>,
    previous: Option<String>,
    labels: Vec<(String, String)>,
    evidence: Evidence,
}

impl Ready {
    fn new(write: Write, permit: Permit, group: &Group) -> Self {
        Self { write, permit, group_name: g::label(&group.name, 64), role: None, post: None, member: None, art: None, picture: None, offset: 0, file: None, previous: None, labels: Vec::new(), evidence: Evidence::default() }
    }
}

/// The account's own tags, all of them, when VRChat gave them for this very account.
fn own_tags(me: &serde_json::Value, user: &str) -> Option<Vec<String>> {
    if me.get("id").and_then(|i| i.as_str()) != Some(user) {
        return None;
    }
    me.get("tags")?.as_array()?.iter().map(|t| t.as_str().map(str::to_string)).collect()
}

fn fill_card(card: &mut GroupCard, group: &Group, me: &str) {
    card.name = group.name.clone();
    card.short_code = group.short_code.clone();
    card.discriminator = group.discriminator.clone();
    card.icon_url = group.icon_url.clone();
    card.banner_url = group.banner_url.clone();
    card.member_count = group.member_count;
    card.online_count = group.online_count;
    card.owner = card.owner || group.owner_id == me;
    card.named = true;
}

/// Why the record did not take it.
fn not(n: Not) -> Failure {
    match n {
        Not::Kept => Failure::NotKept,
        Not::Owner => Failure::OtherOwner,
        Not::Read => Failure::RecordUnreadable,
    }
}

/// The account's own membership as a member's record: only with both its role arrays whole, of this group.
fn own_record(group: &Group) -> Option<Member> {
    let me = group.my_member.as_ref().filter(|me| me.roles_known && me.managed_known && me.group_id.as_deref().is_none_or(|g| g == group.id))?;
    Some(Member { user_id: me.user_id.clone(), membership_id: me.membership_id.clone(), joined_at: me.joined_at.clone(), role_ids: me.role_ids.clone(), roles_known: true, group_id: Some(group.id.clone()), ..Default::default() })
}

/// The file is the account's own and of the kind asked, by what VRChat says of it: not said is not known (verify3 Major 4).
fn own_picture(file: &GroupFile, tag: FileTag, me: &str) -> Result<(), Failure> {
    let tags = file.tags.as_ref().filter(|_| !file.owner.is_empty()).ok_or(Failure::NotRead(Missing::Picture))?;
    if file.owner != me || !tags.iter().any(|t| t == tag.wire()) {
        return Err(Failure::Withheld(Why::NotYourPicture));
    }
    Ok(())
}

fn scope_user(scope: &Scope) -> Result<String, Failure> {
    scope.vrchat_user().map(str::to_string).ok_or(Failure::SignedOut)
}

/// A post as it goes out: an audience this version cannot carry whole is refused, never narrowed or widened quietly
/// (Blocker 3).
fn clean_post(draft: PostDraft) -> Result<PostDraft, Failure> {
    let title = draft.title.trim().to_string();
    let text = draft.text.trim().to_string();
    if title.is_empty() || text.is_empty() || title.chars().count() > g::POST_TITLE_MAX || text.chars().count() > g::POST_TEXT_MAX {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    if draft.role_ids.len() > 32 || draft.role_ids.iter().any(|r| !g::is_role_id(r)) {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    // Public on purpose: the restriction chosen before goes with the switch (N3); the confirmation shows none.
    let role_ids = draft.audience();
    Ok(PostDraft { title, text, role_ids, ..draft })
}

#[cfg(test)]
#[path = "groups_world.rs"]
pub(crate) mod world;

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
