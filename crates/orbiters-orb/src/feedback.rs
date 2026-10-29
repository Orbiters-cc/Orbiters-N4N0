//! World feedback in N4N0's menu (contracts `Docs/orb.md`, Menu, from screen version 13; `Docs/world-feedback.md`): the
//! worlds the menus give feedback on (the one the player is in, the last capture's, previous instances'), each world's
//! likes, labels and comments as the host knows them, and what the menus ask the host to send. The desktop reads and
//! sends (through the same queue as the N4N0 world's lines); what the menus show of it is decided here.

use orbiters_contracts::world_feedback::{self as contract, Label};
use serde::{Deserialize, Serialize};

/// Labels the World feedback page shows, the most given first.
pub const TOP_LABELS: usize = 6;
/// Comments on a page of Comments.
pub const COMMENTS_PER_PAGE: usize = 3;
/// Worlds World feedback steps through at most.
pub const SUBJECTS_MAX: usize = 12;

/// Why a world is among those the menus give feedback on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum Why {
    /// The player is in it now.
    #[default]
    Here,
    /// The last 3D capture is of it.
    Capture,
    /// A previous instance was of it, joined then (Unix seconds).
    Instance { at: u64 },
}

/// A world the menus give feedback on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub world: String,
    pub name: String,
    pub why: Why,
}

/// A label's count for a world, and whether the player gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelCount {
    pub id: String,
    pub count: u32,
    pub mine: bool,
}

/// A comment as the menus show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRow {
    /// The server's id; 0 while it is on its way.
    pub id: u64,
    pub by: String,
    pub text: String,
    /// When it was written (Unix seconds).
    pub at: u64,
    pub mine: bool,
}

/// One world's feedback, the player's own part included (with what is not sent yet).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldFeedback {
    pub world: String,
    /// Not read from Orbiters yet.
    pub loading: bool,
    pub likes: u32,
    pub liked: bool,
    /// Every label with a count or given by the player.
    pub labels: Vec<LabelCount>,
    /// The player's justification of the label that needs one.
    #[serde(default)]
    pub justification: Option<String>,
    /// The newest comments, newest first, and how many there are.
    pub comments: Vec<CommentRow>,
    pub comment_count: u32,
    /// The player's actions not sent yet.
    #[serde(default)]
    pub pending: u32,
    /// The last action refused, or why it could not be read.
    #[serde(default)]
    pub error: Option<String>,
}

impl WorldFeedback {
    /// A world not read yet.
    pub fn loading(world: &str) -> Self {
        Self { world: world.into(), loading: true, ..Default::default() }
    }

    /// The label's count and the player's part.
    pub fn label(&self, id: &str) -> Option<&LabelCount> {
        self.labels.iter().find(|l| l.id == id)
    }

    pub fn has(&self, id: &str) -> bool {
        self.label(id).is_some_and(|l| l.mine)
    }

    /// The labels people gave most, the player's among them: highest count first, then the list's order.
    pub fn top(&self) -> Vec<&LabelCount> {
        let order = |id: &str| contract::labels().iter().position(|l| l.id == id).unwrap_or(usize::MAX);
        let mut shown: Vec<&LabelCount> = self.labels.iter().filter(|l| (l.count > 0 || l.mine) && contract::label(&l.id).is_some()).collect();
        shown.sort_by(|a, b| b.count.cmp(&a.count).then(order(&a.id).cmp(&order(&b.id))));
        shown.truncate(TOP_LABELS);
        shown
    }

    /// The player likes it (or no longer): the count follows at once.
    pub fn set_like(&mut self, on: bool) {
        if self.liked != on {
            self.liked = on;
            self.likes = if on { self.likes + 1 } else { self.likes.saturating_sub(1) };
        }
    }

    /// The player gives the label (or takes it back): its count follows at once.
    pub fn set_label(&mut self, id: &str, on: bool) {
        match self.labels.iter_mut().find(|l| l.id == id) {
            Some(l) if l.mine != on => {
                l.mine = on;
                l.count = if on { l.count + 1 } else { l.count.saturating_sub(1) };
            }
            Some(_) => {}
            None if on => self.labels.push(LabelCount { id: id.into(), count: 1, mine: true }),
            None => {}
        }
    }
}

/// What the host knows: whether feedback can be given, the worlds, and each one's feedback read so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackView {
    /// An Orbiters account is signed in (feedback can be given).
    pub signed_in: bool,
    /// The worlds to step through, the one the player is in first.
    pub subjects: Vec<Subject>,
    pub worlds: Vec<WorldFeedback>,
}

impl FeedbackView {
    pub fn world(&self, id: &str) -> Option<&WorldFeedback> {
        self.worlds.iter().find(|w| w.world == id)
    }

    pub fn world_mut(&mut self, id: &str) -> &mut WorldFeedback {
        match self.worlds.iter().position(|w| w.world == id) {
            Some(i) => &mut self.worlds[i],
            None => {
                self.worlds.push(WorldFeedback::loading(id));
                self.worlds.last_mut().expect("just pushed")
            }
        }
    }
}

/// What the menus ask the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum FeedbackAsk {
    /// A world's feedback shows: read it (again).
    Show {
        world: String,
    },
    Like {
        world: String,
        on: bool,
    },
    Label {
        world: String,
        label: String,
        on: bool,
        text: Option<String>,
    },
    Comment {
        world: String,
        text: String,
    },
    Uncomment {
        world: String,
        comment: u64,
    },
}

/// The label of the list at `i` (the labels page's chips, in the list's order).
pub fn label_at(i: usize) -> Option<&'static Label> {
    contract::labels().get(i)
}

/// Where the label `id` is in the list (its chip on the labels page).
pub fn label_index(id: &str) -> Option<u8> {
    contract::labels().iter().position(|l| l.id == id).map(|i| i as u8)
}

/// What a world's line under its name says: why it is there.
pub fn why_text(why: Why, now: u64) -> String {
    match why {
        Why::Here => "The world you are in".into(),
        Why::Capture => "The world of your last 3D capture".into(),
        Why::Instance { at } => format!("You were here {}", ago(now.saturating_sub(at))),
    }
}

/// `5 min ago`, `3 h ago`, `2 days ago`.
pub fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// `now`, `5 min`, `3 h`, `2 d`: [`ago`] where room is short.
pub fn ago_short(seconds: u64) -> String {
    match seconds {
        0..=59 => "now".into(),
        60..=3599 => format!("{} min", seconds / 60),
        3600..=86_399 => format!("{} h", seconds / 3600),
        _ => format!("{} d", seconds / 86_400),
    }
}

/// `1 like`, `12 likes`.
pub fn likes_text(n: u32) -> String {
    if n == 1 {
        "1 like".into()
    } else {
        format!("{n} likes")
    }
}

/// `No comment yet`, `1 comment`, `7 comments`.
pub fn comments_text(n: u32) -> String {
    match n {
        0 => "No comment yet".into(),
        1 => "1 comment".into(),
        n => format!("{n} comments"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn likes_and_labels_follow_the_player_at_once() {
        let mut w = WorldFeedback { world: "wrld_x".into(), likes: 4, ..Default::default() };
        w.set_like(true);
        w.set_like(true);
        assert_eq!((w.likes, w.liked), (5, true));
        w.set_like(false);
        assert_eq!((w.likes, w.liked), (4, false));
        w.set_label("mirror", true);
        w.labels.push(LabelCount { id: "meetup".into(), count: 3, mine: false });
        w.labels.push(LabelCount { id: "pens".into(), count: 1, mine: false });
        w.labels.push(LabelCount { id: "not-a-label".into(), count: 9, mine: false });
        let top: Vec<&str> = w.top().iter().map(|l| l.id.as_str()).collect();
        assert_eq!(top, ["meetup", "mirror", "pens"], "highest first, the list's order between equals, unknown ids left out");
        w.set_label("mirror", false);
        assert!(w.top().iter().all(|l| l.id != "mirror"));
    }

    #[test]
    fn times_read_shortly() {
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
        assert_eq!(ago(90_000), "yesterday");
        assert_eq!(why_text(Why::Instance { at: 100 }, 100 + 3 * 86_400), "You were here 3 days ago");
        assert_eq!((likes_text(1), likes_text(0), comments_text(7)), ("1 like".into(), "0 likes".into(), "7 comments".into()));
    }
}
