//! Previous instances (N4N0's menu, More): the instances the player was in, read from the visits Orbiters - N4N0 keeps
//! from VRChat's logs (`orbiters_core::visited`), and Invite me, through which Orbiters' own VRChat account invites the
//! signed-in member's VRChat account to one of them (`POST /n4n0/invites`). VRChat lets an account invite only its
//! friends: until the member added it, Orbiters answers with the account to add.

use crate::menu::{InstanceRow, InviteAccount, InviteAnswer};
use orbiters_core::account::Api;
use orbiters_core::visited::History;
use serde::Deserialize;
use std::time::Duration;

/// How long an invite waits for Orbiters (it asks VRChat in turn).
const TIMEOUT: Duration = Duration::from_secs(20);

/// The instances of `history`, the newest first, as Previous instances lists them.
pub fn rows(history: &History) -> Vec<InstanceRow> {
    history
        .recent_instances()
        .into_iter()
        .map(|(visit, name)| InstanceRow { location: visit.location(), world: name.unwrap_or_default(), at: visit.at })
        .collect()
}

/// The instances kept in this computer's `visited-worlds.json` (the tray service writes it as VRChat's logs grow).
pub fn read_rows() -> Vec<InstanceRow> {
    rows(&History::load(&orbiters_core::visited::history_file()))
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
        Err(e) => return InviteAnswer::Failed { message: format!("Orbiters could not be reached: {e}") },
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
    let message = if body.message.trim().is_empty() { format!("Orbiters answered {status}") } else { body.message.trim().to_string() };
    match (body.code.as_str(), body.account) {
        ("not_friends" | "accepting", Some(account)) if !account.name.is_empty() => {
            InviteAnswer::NeedsFriend { account: InviteAccount { name: account.name, link: account.link }, message }
        }
        ("unauthorized", _) if status == 401 => InviteAnswer::Failed { message: "Sign in to Orbiters in Orbiters - N4N0 first (Settings)".into() },
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
}
