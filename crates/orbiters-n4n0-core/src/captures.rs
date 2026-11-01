//! 3D captures shared from N4N0's menu (contracts `Docs/n4n0.md`, Menu, from screen version 12; `Docs/capture-sharing.md`):
//! the captures the menus list, where each stands (its 3D view, its shares), and the friends a capture goes to. The
//! desktop lists the captures, reads the friends and shares; what the menus show of them is decided here.

use serde::{Deserialize, Serialize};

/// Captures History lists at most, the newest.
pub const HISTORY_MAX: usize = 60;
/// Friends one share goes to at most (the sharing API's limit for a call).
pub const RECIPIENTS_MAX: usize = 50;
/// Characters of a friend's name to find.
pub const FIND_MAX: usize = 32;
/// A capture's picture is known to the menus by this and its key ([`CaptureRow::picture_key`]).
pub const PICTURE_PREFIX: &str = "capture:";
/// What the server and the menus say before an explicit capture is shared publicly (`Docs/capture-sharing.md`, Rules).
pub const EXPLICIT_WARNING: &str = "Any explicit content shared publicly will be reported to the VRChat staff.";

/// Where a capture's 3D view stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum CapturePhase {
    /// Being recorded.
    Recording,
    /// Its files being finished.
    Saving,
    /// Waiting for its turn to be reconstructed.
    Waiting,
    /// Its 3D view being made, this far.
    Making { percent: u8 },
    /// Its 3D view is made: it can be shared.
    #[default]
    Ready,
    /// Not reconstructed, nor waiting to be (a share starts it).
    NoView,
}

/// Where the last share asked for a capture stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Sharing {
    #[default]
    Idle,
    /// Asked before its 3D view was made: it goes once the view is (publicly, and to so many friends).
    Waiting { public: bool, people: u32 },
    /// Being prepared, uploaded and shared.
    Busy { public: bool, people: u32 },
    /// Shared (publicly, and to so many friends).
    Done { public: bool, people: u32 },
    /// The server asks to acknowledge its explicit content first (`explicit-acknowledgement`).
    Explicit,
    /// It could not be shared: why.
    Failed { message: String },
}

/// A 3D capture as the menus show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRow {
    /// The desktop's own name for it (its session's folder): shares and pictures name it by this.
    pub key: String,
    pub world: String,
    /// When it was taken (Unix seconds).
    pub taken: u64,
    pub phase: CapturePhase,
    /// Flagged explicit when it was taken (`capture-content.json`) or by the server, and why.
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub explicit_reasons: Vec<String>,
    /// Listed publicly now, and given to so many people.
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub people: u32,
    #[serde(default)]
    pub sharing: Sharing,
}

impl CaptureRow {
    /// The key its picture is kept under by the menus.
    pub fn picture_key(&self) -> String {
        format!("{PICTURE_PREFIX}{}", self.key)
    }

    /// What it is at, in a few words (History's rows hold 20 characters after when it was taken): its last share, else
    /// its 3D view while it is not made, else where it is shared.
    pub fn state(&self) -> String {
        let whom = |public: bool, n: u32| match (public, n) {
            (true, 0) => "Shared publicly".to_string(),
            (true, n) => format!("Public · {n} friend{}", if n == 1 { "" } else { "s" }),
            (false, n) => format!("With {n} friend{}", if n == 1 { "" } else { "s" }),
        };
        match &self.sharing {
            Sharing::Waiting { .. } => return "Shares when ready".into(),
            Sharing::Busy { .. } => return "Sharing…".into(),
            Sharing::Done { public, people } => return whom(*public, *people),
            Sharing::Explicit => return "Explicit: confirm".into(),
            Sharing::Failed { .. } => return "Not shared".into(),
            Sharing::Idle => {}
        }
        match self.phase {
            CapturePhase::Recording => "Recording…".into(),
            CapturePhase::Saving => "Saving…".into(),
            CapturePhase::Waiting => "Waiting for 3D view".into(),
            CapturePhase::Making { percent } => format!("3D view · {percent} %"),
            CapturePhase::NoView => "No 3D view yet".into(),
            CapturePhase::Ready if self.public || self.people > 0 => whom(self.public, self.people),
            CapturePhase::Ready => "Ready to share".into(),
        }
    }

    /// A share of it is on its way (asked, waiting for its view, or being made).
    pub fn busy(&self) -> bool {
        matches!(self.sharing, Sharing::Waiting { .. } | Sharing::Busy { .. })
    }

    /// It can be shared now or once its view is made: not still being recorded, nor a share on its way.
    pub fn shareable(&self) -> bool {
        !matches!(self.phase, CapturePhase::Recording | CapturePhase::Saving) && !self.busy()
    }
}

/// The captures as the desktop lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturesView {
    /// The last capture the desktop made since it started (3D picture or recording).
    #[serde(default)]
    pub latest: Option<CaptureRow>,
    /// The computer's captures, newest first (at most [`HISTORY_MAX`]), once listed.
    #[serde(default)]
    pub history: Vec<CaptureRow>,
    #[serde(default)]
    pub listed: bool,
    /// Why there are none, or what went wrong listing them.
    #[serde(default)]
    pub note: String,
}

impl CapturesView {
    /// The capture named `key`: the last one, or one listed.
    pub fn find(&self, key: &str) -> Option<&CaptureRow> {
        self.latest.iter().chain(&self.history).find(|c| c.key == key)
    }
}

/// A VRChat friend a capture can go to.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Friend {
    /// Their VRChat user id (`usr_…`).
    pub id: String,
    pub name: String,
}

/// The friends as the desktop read them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Friends {
    /// Not asked for.
    #[default]
    None,
    Loading,
    /// By name.
    Ready {
        friends: Vec<Friend>,
    },
    Failed {
        message: String,
    },
}

impl Friends {
    /// The friends known (none while loading or failed).
    pub fn list(&self) -> &[Friend] {
        match self {
            Friends::Ready { friends } => friends,
            _ => &[],
        }
    }

    /// Ready, sorted by name (case aside), each friend once and only VRChat user ids.
    pub fn ready(mut friends: Vec<Friend>) -> Self {
        let mut seen = std::collections::HashSet::new();
        friends.retain(|f| f.id.starts_with("usr_") && seen.insert(f.id.clone()));
        friends.sort_by_cached_key(|f| (f.name.to_lowercase(), f.id.clone()));
        Friends::Ready { friends }
    }
}

/// A share the menu asks of the desktop.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareAsk {
    /// The capture's key ([`CaptureRow::key`]).
    pub key: String,
    pub public: bool,
    #[serde(default)]
    pub recipients: Vec<Friend>,
    /// The owner saw [`EXPLICIT_WARNING`] and shares anyway.
    #[serde(default)]
    pub acknowledge_explicit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_says_where_its_view_and_its_shares_are() {
        let mut row = CaptureRow { key: "s".into(), phase: CapturePhase::Making { percent: 42 }, ..Default::default() };
        assert_eq!(row.state(), "3D view · 42 %");
        assert!(row.shareable(), "a share waits for its view");
        row.sharing = Sharing::Waiting { public: true, people: 0 };
        assert_eq!(row.state(), "Shares when ready");
        assert!(!row.shareable() && row.busy());
        row = CaptureRow { phase: CapturePhase::Ready, ..row };
        row.sharing = Sharing::Done { public: true, people: 2 };
        assert_eq!(row.state(), "Public · 2 friends");
        row.sharing = Sharing::Idle;
        assert_eq!(row.state(), "Ready to share");
        row.people = 1;
        assert_eq!(row.state(), "With 1 friend");
        row.sharing = Sharing::Failed { message: "Sign in to Orbiters in Settings first.".into() };
        assert_eq!(row.state(), "Not shared");
        // Every state fits History's rows.
        for sharing in [Sharing::Waiting { public: true, people: 0 }, Sharing::Done { public: true, people: 50 }, Sharing::Explicit] {
            assert!(CaptureRow { sharing, ..Default::default() }.state().chars().count() <= 20);
        }
        assert!(!CaptureRow { phase: CapturePhase::Recording, ..Default::default() }.shareable());
    }

    #[test]
    fn friends_are_listed_once_each_by_name() {
        let f = |id: &str, name: &str| Friend { id: id.into(), name: name.into() };
        let friends = Friends::ready(vec![f("usr_b", "bob"), f("usr_a", "Alice"), f("usr_b", "Bob again"), f("grp_x", "Group")]);
        assert_eq!(friends.list(), [f("usr_a", "Alice"), f("usr_b", "bob")]);
        assert!(Friends::Loading.list().is_empty());
    }

    #[test]
    fn the_status_keeps_its_json() {
        let row = CaptureRow {
            key: "k".into(),
            sharing: Sharing::Busy { public: false, people: 3 },
            phase: CapturePhase::Making { percent: 7 },
            ..Default::default()
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(
            json.contains(r#""phase":{"phase":"making","percent":7}"#) && json.contains(r#""sharing":{"state":"busy","public":false,"people":3}"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<CaptureRow>(&json).unwrap(), row);
        let friends = serde_json::to_string(&Friends::Failed { message: "x".into() }).unwrap();
        assert_eq!(friends, r#"{"phase":"failed","message":"x"}"#);
    }
}
