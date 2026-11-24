//! Previous instances (N4N0's menu, More): the instances the player was in, read from the visits Orbiters - N4N0 keeps
//! from VRChat's logs (`orbiters_core::visited`), and Invite me, through which Orbiters' own VRChat account invites the
//! signed-in member's VRChat account to one of them (`POST /n4n0/invites`). VRChat lets an account invite only its
//! friends: until the member added it, Orbiters answers with the account to add.

use crate::menu::{InstanceRow, InviteAccount, InviteAnswer};
use crate::text::latin;
use orbiters_core::account::Api;
use orbiters_i18n::{tr, tr_latin};
use orbiters_core::visited::{History, Viewer};
use serde::Deserialize;
use std::time::Duration;

/// How long an invite waits for Orbiters (it asks VRChat in turn).
const TIMEOUT: Duration = Duration::from_secs(20);

/// The instances of `history`, the newest first, as Previous instances lists them.
pub fn rows(history: &History) -> Vec<InstanceRow> {
    history
        .recent_instances()
        .into_iter()
        .map(|(visit, name)| InstanceRow { location: visit.location(), world: name.unwrap_or_default(), at: visit.at, account: visit.account })
        .collect()
}

/// Those of `rows`, listed for another before, that `viewer` is shown as they are (`History::seen_by` reads them anew).
pub fn shown(rows: Vec<InstanceRow>, viewer: &Viewer) -> Vec<InstanceRow> {
    let public = |row: &InstanceRow| row.location.split_once(':').is_some_and(|(_, instance)| orbiters_core::vrchat_launch::is_public_instance(instance));
    rows.into_iter().filter(|row| viewer.lists(row.account.as_deref()) && (row.account.is_some() || viewer.single || public(row))).collect()
}

/// Whether the place `location` of `rows` may be sent anywhere for the VRChat account `me` (`orbiters_core::visited::goes_for`).
pub fn goes_for(rows: &[InstanceRow], location: &str, me: &str) -> bool {
    let instance = location.split_once(':').map_or("", |(_, instance)| instance);
    rows.iter().any(|row| row.location == location && orbiters_core::visited::goes_for(row.account.as_deref(), instance, me))
}

/// Whether the place `location` may be sent to Orbiters, for its VRChat account to invite the member's: a public one. Which
/// VRChat account is the member's is not known here, so a private one, which is one account's, is not.
pub fn goes_to_orbiters(location: &str) -> bool {
    location.split_once(':').is_some_and(|(_, instance)| orbiters_core::vrchat_launch::is_public_instance(instance))
}

/// The instances kept in this computer's `visited-worlds.json` (the tray service writes it as VRChat's logs grow), as `viewer`
/// is shown them.
pub fn read_rows(viewer: &Viewer) -> Vec<InstanceRow> {
    rows(&History::load_for(&orbiters_core::visited::history_file(), viewer))
}

#[derive(Deserialize, Default)]
struct Answer {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    account: Option<Account>,
}

#[derive(Deserialize, Default)]
struct Account {
    #[serde(default)]
    name: String,
    #[serde(default)]
    link: String,
}

/// Has Orbiters' VRChat account invite the account of `token`'s member to `location` (`wrld_…:<instance>`).
pub fn send(api: &Api, token: &str, location: &str) -> InviteAnswer {
    let response = api
        .agent()
        .post(&api.url("n4n0/invites"))
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "instance": location }));
    let response = match response {
        Ok(r) => r,
        Err(e) => return InviteAnswer::Failed { message: latin(|| tr!("Orbiters could not be reached: {error}", error = e)) },
    };
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().unwrap_or_default();
    answer(status, &text)
}

/// What Orbiters' answer `text` with `status` means for the menu.
fn answer(status: u16, text: &str) -> InviteAnswer {
    let body: Answer = serde_json::from_str(text).unwrap_or_default();
    if (200..300).contains(&status) {
        return InviteAnswer::Sent;
    }
    let message = if body.message.trim().is_empty() { latin(|| tr!("Orbiters answered {status}", status = status)) } else { body.message.trim().to_string() };
    match (body.code.as_str(), body.account) {
        ("not_friends" | "accepting", Some(account)) if !account.name.is_empty() => {
            InviteAnswer::NeedsFriend { account: InviteAccount { name: account.name, link: account.link }, message }
        }
        ("unauthorized", _) if status == 401 => {
            let message = tr_latin("Sign in to Orbiters in Orbiters - N4N0 first (Settings)").to_string();
            InviteAnswer::Failed { message }
        }
        _ => InviteAnswer::Failed { message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbiters_answers_become_what_the_menu_shows() {
        assert_eq!(answer(201, r#"{"sent":true,"account":{"name":"orbiters","vrchatId":"usr_1","link":"https://vrchat.com/home/user/usr_1"}}"#), InviteAnswer::Sent);
        let befriend = answer(
            409,
            r#"{"code":"not_friends","message":"Add Orbiters' VRChat account as a friend on VRChat first, then try again.","account":{"name":"orbiters","vrchatId":"usr_1","link":"https://vrchat.com/home/user/usr_1"}}"#,
        );
        assert_eq!(
            befriend,
            InviteAnswer::NeedsFriend {
                account: InviteAccount { name: "orbiters".into(), link: "https://vrchat.com/home/user/usr_1".into() },
                message: "Add Orbiters' VRChat account as a friend on VRChat first, then try again.".into(),
            }
        );
        assert!(matches!(answer(409, r#"{"code":"accepting","message":"m","account":{"name":"orbiters","link":"l"}}"#), InviteAnswer::NeedsFriend { .. }));
        assert_eq!(answer(409, r#"{"code":"not_linked","message":"Link your VRChat account on Orbiters first."}"#), InviteAnswer::Failed {
            message: "Link your VRChat account on Orbiters first.".into()
        });
        assert_eq!(answer(503, "<html>"), InviteAnswer::Failed { message: "Orbiters answered 503".into() });
        assert!(matches!(answer(401, r#"{"code":"unauthorized","message":"x"}"#), InviteAnswer::Failed { message } if message.contains("Sign in")));
    }

    #[test]
    fn rows_are_the_history_s_instances_newest_first_with_their_world() {
        let mut history = History::default();
        history.record("wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f", 100);
        history.record_instance("wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f", "1~region(eu)", 100);
        history.record_instance("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b", "2~hidden(usr_x)", 200);
        let rows = rows(&history);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].location, "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:2~hidden(usr_x)");
        assert_eq!((rows[0].kind(), rows[0].region()), ("Friends+", None));
        assert_eq!((rows[1].kind(), rows[1].region(), rows[1].at), ("Public", Some("EU"), 100));
    }

    /// R3: a previous instance is its account's: shown under no other, sent for no other, and a private one never to Orbiters'
    /// account, also where one account's alone are kept (the member's is not known to be that one). A private one that is nobody's
    /// goes nowhere, and is shown without its private part.
    #[test]
    fn a_previous_instance_goes_for_the_account_it_came_from_alone() {
        const WORLD: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
        let mut history = History::default();
        history.record_instance_by(WORLD, "1~friends(usr_z)", 1, Some("usr_x"));
        history.record_instance_by(WORLD, "2~region(eu)", 2, Some("usr_x"));
        history.record_instance_by(WORLD, "3~friends(usr_z)", 3, None);
        history.record_instance_by(WORLD, "4~region(eu)", 4, None);
        let rows = rows(&history);
        let at = |instance: &str| format!("{WORLD}:{instance}");
        let shown_as = |acting: &[&str], single: bool| super::rows(&history.clone().seen_by(&Viewer { acting: acting.iter().map(|a| a.to_string()).collect(), single }));
        let times = |acting: &[&str], single: bool| shown_as(acting, single).iter().map(|r| r.at).collect::<Vec<_>>();
        assert_eq!(times(&["usr_x"], false), [4, 3, 2, 1]);
        assert_eq!(times(&["usr_y"], false), [4, 3]);
        assert_eq!(times(&["usr_x", "usr_y"], false), [4, 3]);
        assert_eq!((times(&[], false), times(&[], true)), (vec![4, 3], vec![4, 3, 2, 1]));
        let bare = |single: bool| shown_as(&["usr_x"], single)[1].location.clone();
        assert_eq!((bare(false), bare(true)), (format!("{WORLD}:"), at("3~friends(usr_z)")));
        for (instance, x, y) in [("1~friends(usr_z)", true, false), ("2~region(eu)", true, false), ("3~friends(usr_z)", false, false), ("4~region(eu)", true, true), ("5~region(eu)", false, false)] {
            assert_eq!((goes_for(&rows, &at(instance), "usr_x"), goes_for(&rows, &at(instance), "usr_y")), (x, y), "{instance}");
        }
        assert_eq!(["1~friends(usr_z)", "3~friends(usr_z)", "4~region(eu)", ""].map(|instance| goes_to_orbiters(&at(instance))), [false, false, true, false]);
    }
}
