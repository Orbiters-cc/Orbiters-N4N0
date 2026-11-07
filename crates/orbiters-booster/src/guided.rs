//! Step 3's list: VRChat's own limits on other people's avatars, one at a time, changed by the person in VRChat's menu. The
//! booster changes nothing here. VRChat offers no general way to set these from outside, and a change by hand is also the
//! only one that can be measured in the same crowd: pressing Done marks the moment, and thirty seconds before are compared
//! with thirty seconds after. Those figures are labelled as what they are: the person changed this, and people may have
//! moved.

use crate::classify::Limit;
use serde::{Deserialize, Serialize};

/// One thing to change in VRChat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Item {
    /// Maximum Shown Avatars: 15.
    MaxShown,
    /// Avatar Performance: block Very Poor.
    BlockVeryPoor,
    /// Hide Avatars Beyond: 10 m.
    HideBeyond,
    /// MSAA: 2x.
    Msaa2x,
    /// MSAA: Off.
    MsaaOff,
}

/// In what order, for what limits this PC. Hiding avatars saves their rendering, but VRChat says their animators keep
/// running, so on the processor the gain is unknown: the list is what can be tried, not what will work.
pub fn list_for(limit: Limit) -> Vec<Item> {
    let avatars = [Item::MaxShown, Item::BlockVeryPoor, Item::HideBeyond];
    match limit {
        Limit::Processor | Limit::Both => avatars.to_vec(),
        Limit::GraphicsCard => [Item::Msaa2x, Item::MsaaOff].into_iter().chain(avatars).collect(),
        // "Nothing here is likely to help right now": the list can still be opened.
        Limit::Fine | Limit::NotSure => avatars.to_vec(),
    }
}

/// Whether the list is likely to help now.
pub fn likely_to_help(limit: Limit) -> bool {
    matches!(limit, Limit::Processor | Limit::Both | Limit::GraphicsCard)
}

/// The thirty seconds before Done, and after.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    pub item: Item,
    pub before_fps: f32,
    pub after_fps: f32,
    pub before_filled: f32,
    pub after_filled: f32,
    pub before_people: Option<u32>,
    pub after_people: Option<u32>,
}

/// Where the person is in the list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub at: usize,
    pub items: Vec<Item>,
    /// Being measured after Done, and how many seconds are left.
    pub measuring: Option<(Item, f32)>,
    /// Every Done measured: repeating an item, changed back and again, gives a real A/B to compare.
    pub results: Vec<Measured>,
    pub likely: bool,
}

impl Guide {
    pub fn item(&self) -> Option<Item> {
        self.items.get(self.at).copied()
    }
}

/// What one limit on avatars is set to, for the list and for putting it back. Read from the registry, read-only
/// ([`crate::vrchat_read`]), when VRChat's account is known; otherwise the person is told to look in VRChat's menu.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Known {
    Off,
    Count(u32),
    Meters(f32),
}

/// What a person changed by hand and should have back when they leave step 3: the booster does not undo their own changes,
/// it shows the list, until each is back.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToPutBack {
    pub item: Item,
    /// What it was. `None`: the booster could not read it, so it says to check VRChat's menu.
    pub was: Option<Known>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_follows_the_limit() {
        assert_eq!(list_for(Limit::Processor), vec![Item::MaxShown, Item::BlockVeryPoor, Item::HideBeyond]);
        assert_eq!(list_for(Limit::GraphicsCard)[..2], [Item::Msaa2x, Item::MsaaOff]);
        assert_eq!(list_for(Limit::GraphicsCard).len(), 5);
        assert!(!likely_to_help(Limit::Fine) && !likely_to_help(Limit::NotSure));
        assert!(likely_to_help(Limit::Both));
    }
}
