//! The words the interface and the notification show, from the snapshot.

use crate::app::{AccountState, Link, Snapshot};
use crate::connection::Phase;
use orbiters_n4n0::status::{StateView, StatesView};
use time::OffsetDateTime;

/// The notification's line: how N4N0 stands with the Quest.
pub fn notification_line(s: &Snapshot) -> String {
    match s.link.phase {
        Phase::Connected => match s.orb.avatar_id.as_deref() {
            Some(id) => format!("Connected to VRChat · {}", avatar_label(id)),
            None => "Connected to VRChat on the Quest".into(),
        },
        Phase::Found => "VRChat found, waiting for it to answer".into(),
        Phase::Searching | Phase::Starting => "Looking for VRChat on the Wi-Fi".into(),
        Phase::NoWifi => "Waiting for the Wi-Fi".into(),
        Phase::Failed => "OSC could not start".into(),
    }
}

/// The connection card's title.
pub fn link_title(link: &Link) -> String {
    match link.phase {
        Phase::Connected => "Connected to VRChat".into(),
        Phase::Found => "VRChat found".into(),
        Phase::Searching => "Looking for VRChat".into(),
        Phase::Starting => "Starting".into(),
        Phase::NoWifi => "No Wi-Fi".into(),
        Phase::Failed => "OSC could not start".into(),
    }
}

/// The connection card's line under the title.
pub fn link_detail(link: &Link) -> String {
    match link.phase {
        Phase::Connected => match (link.quest, link.quest_name.as_deref()) {
            (Some(ip), _) => format!("On the Quest at {ip}"),
            (None, Some(name)) => name.to_string(),
            (None, None) => "On the Quest".into(),
        },
        Phase::Found => match link.quest {
            Some(ip) if link.typed => format!("Sending to {ip}; nothing came back yet"),
            Some(ip) => format!("At {ip}; nothing came back yet"),
            None => "Nothing came back yet".into(),
        },
        Phase::Searching => "On this Wi-Fi, with OSC on in VRChat".into(),
        Phase::Starting => "Opening N4N0's OSC services".into(),
        Phase::NoWifi => "Join the Wi-Fi your Quest is on".into(),
        Phase::Failed => link.error.clone().unwrap_or_else(|| "Unknown error".into()),
    }
}

/// What to do about it, when something is to be done.
pub fn link_hint(link: &Link) -> String {
    match link.phase {
        Phase::Searching | Phase::Found => {
            "In VRChat on the Quest, open the Action Menu, then Options › OSC › Enabled. If OSC was on already, turn it off and on, or type the Quest's address in Settings.".into()
        }
        Phase::NoWifi => "The phone and the Quest must be on the same Wi-Fi (or the Quest on this phone's hotspot).".into(),
        Phase::Failed => "Try Look again, or turn the Wi-Fi off and on.".into(),
        _ => String::new(),
    }
}

/// The phone's address and network (`192.168.1.20 · wlan0`).
pub fn phone_line(link: &Link) -> String {
    match &link.phone {
        Some((name, ip)) => format!("{ip} · {name}"),
        None => "Not on a local network".into(),
    }
}

/// An avatar id cut for a line: `avtr_1a2b3c4d…9f0e`.
pub fn avatar_label(id: &str) -> String {
    let rest = id.strip_prefix("avtr_").unwrap_or(id);
    let chars: Vec<char> = rest.chars().collect();
    if chars.len() <= 14 {
        return id.to_string();
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("avtr_{head}…{tail}")
}

/// The avatar line on Home and the States page.
pub fn avatar_line(s: &Snapshot) -> String {
    match (s.link.phase, s.orb.avatar_id.as_deref().or(s.orb.states.avatar_id.as_deref())) {
        (_, Some(id)) => avatar_label(id),
        (Phase::Connected, None) => "Reading the avatar…".into(),
        _ => "No avatar yet".into(),
    }
}

/// N4N0 on the avatar worn.
pub fn n4n0_line(s: &Snapshot) -> String {
    if s.orb.avatar_id.is_none() {
        return "Waiting for an avatar".into();
    }
    match (s.orb.prop_sync.is_some(), s.orb.controls) {
        (true, _) => "On this avatar".into(),
        (false, 0) => "Not on this avatar".into(),
        (false, n) => format!("{n} controls on this avatar"),
    }
}

/// The States card's line on Home.
pub fn states_line(v: &StatesView) -> String {
    if v.avatar_id.is_none() {
        return "Save how your avatar is".into();
    }
    let n = v.states.len();
    let default = v.states.iter().find(|s| s.default).map(|s| s.name.as_str());
    match (n, default) {
        (0, _) => "None saved for this avatar".into(),
        (n, Some(d)) => format!("{n} saved · {d} by default"),
        (1, None) => "1 saved".into(),
        (n, None) => format!("{n} saved"),
    }
}

/// A state's line: when it was saved and what it keeps.
pub fn state_line(state: &StateView, now: OffsetDateTime) -> String {
    let when = saved_ago(&state.saved_utc, now);
    let what = match state.parameters {
        1 => "1 parameter".to_string(),
        n => format!("{n} parameters"),
    };
    match when {
        Some(when) => format!("{when} · {what}"),
        None => what,
    }
}

/// `2026-10-25T10:00:00Z` as "Saved 5 min ago" (a date past a week).
pub fn saved_ago(utc: &str, now: OffsetDateTime) -> Option<String> {
    let at = OffsetDateTime::parse(utc, &time::format_description::well_known::Rfc3339).ok()?;
    let secs = (now - at).whole_seconds().max(0);
    Some(match secs {
        0..=59 => "Saved just now".into(),
        60..=3599 => format!("Saved {} min ago", secs / 60),
        3600..=86_399 => format!("Saved {} h ago", secs / 3600),
        86_400..=172_799 => "Saved yesterday".into(),
        172_800..=604_799 => format!("Saved {} days ago", secs / 86_400),
        _ => format!("Saved {:04}-{:02}-{:02}", at.year(), at.month() as u8, at.day()),
    })
}

/// The account card's line.
pub fn account_line(s: &Snapshot) -> String {
    match &s.account {
        AccountState::Checking => "Checking…".into(),
        AccountState::SignedOut => "Not signed in".into(),
        AccountState::SigningIn => "Finish in the browser".into(),
        AccountState::SignedIn { name } => format!("Signed in as {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn avatar_ids_are_cut() {
        assert_eq!(avatar_label("avtr_1a2b3c4d-5e6f-7a8b-9c0d-112233449f0e"), "avtr_1a2b3c4d…9f0e");
        assert_eq!(avatar_label("avtr_short"), "avtr_short");
        assert_eq!(avatar_label("local:123"), "local:123");
    }

    #[test]
    fn saved_times() {
        let now = datetime!(2026-10-25 12:00:00 UTC);
        assert_eq!(saved_ago("2026-10-25T11:59:30Z", now).unwrap(), "Saved just now");
        assert_eq!(saved_ago("2026-10-25T11:55:00Z", now).unwrap(), "Saved 5 min ago");
        assert_eq!(saved_ago("2026-10-25T09:00:00Z", now).unwrap(), "Saved 3 h ago");
        assert_eq!(saved_ago("2026-10-24T09:00:00Z", now).unwrap(), "Saved yesterday");
        assert_eq!(saved_ago("2026-10-21T12:00:00Z", now).unwrap(), "Saved 4 days ago");
        assert_eq!(saved_ago("2026-09-01T12:00:00Z", now).unwrap(), "Saved 2026-09-01");
        assert_eq!(saved_ago("not a time", now), None);
        let state = StateView { saved_utc: "2026-10-25T11:55:00Z".into(), parameters: 42, ..Default::default() };
        assert_eq!(state_line(&state, now), "Saved 5 min ago · 42 parameters");
    }

    #[test]
    fn states_and_links_in_words() {
        let mut v = StatesView::default();
        assert_eq!(states_line(&v), "Save how your avatar is");
        v.avatar_id = Some("avtr_x".into());
        assert_eq!(states_line(&v), "None saved for this avatar");
        v.states.push(StateView { name: "Dance".into(), default: true, ..Default::default() });
        v.states.push(StateView { name: "Sit".into(), ..Default::default() });
        assert_eq!(states_line(&v), "2 saved · Dance by default");
        let mut s = Snapshot::default();
        assert_eq!(notification_line(&s), "Looking for VRChat on the Wi-Fi");
        s.link.phase = Phase::Connected;
        s.link.quest = Some(std::net::Ipv4Addr::new(192, 168, 1, 42));
        assert_eq!(link_detail(&s.link), "On the Quest at 192.168.1.42");
        s.orb.avatar_id = Some("avtr_1a2b3c4d-5e6f-7a8b-9c0d-112233449f0e".into());
        assert_eq!(notification_line(&s), "Connected to VRChat · avtr_1a2b3c4d…9f0e");
        s.link.phase = Phase::NoWifi;
        assert!(link_hint(&s.link).contains("same Wi-Fi"));
    }
}
