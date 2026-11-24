//! Previous instances' Invite me with the person's own VRChat account (REQUEST20 4, Enzo: "it tries to use the Orbiters
//! account to send the invite and always fails. If the user connected his vrchat account in Orbiters N4N0, I would like
//! this one to be used to send the invite instead"). With the VRChat session the app keeps (Settings, VRChat account),
//! VRChat's invite to oneself (`POST /invite/myself/to/{world}:{instance}`) through the app's one client, with its pace
//! and back-off. Only for an instance VRChat's logs say the person was in (the list Invite me is pressed on), looked at
//! first so that a closed, full or barred instance is said as such; nothing is sent but on a press. Without a VRChat
//! session nothing is sent here: N4N0 asks Orbiters' own account instead (`orbiters_n4n0::Host::invite_self`).

use super::Controller;
use orbiters_core::vrchat_api::{Client, Error};
use orbiters_i18n::tr;
use orbiters_n4n0_core::menu::{InstanceRow, InviteAnswer};
use std::sync::Mutex;

impl Controller {
    /// The VRChat account Invite me would go from now (its user id), when one is connected.
    pub(super) fn previous_instance_account(&self) -> Option<String> {
        let client = self.vrchat_signed_client()?;
        let id = client.lock().unwrap().signed_in_user_id().map(str::to_string);
        id
    }

    /// Invite me to `location` from the VRChat account `account`, the one connected as it was asked; `None` when the app has
    /// no VRChat session, or another's by now (nothing sent). On a thread of N4N0's own: it waits for VRChat.
    pub(super) fn previous_instance_invite(&self, location: &str, account: &str) -> Option<InviteAnswer> {
        let client = self.vrchat_signed_client()?;
        if client.lock().unwrap().signed_in_user_id() != Some(account) {
            return None;
        }
        Some(invite_self(&client, account, &orbiters_n4n0_core::invites::read_rows(), location))
    }
}

/// `wrld_…:<instance>` as its two parts, for a location that can only name an instance (no path, nothing odd in it).
pub fn location_parts(location: &str) -> Option<(&str, &str)> {
    let (world, instance) = location.split_once(':')?;
    let world_ok = world.len() <= 64 && world.strip_prefix("wrld_").is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'));
    let instance_ok = !instance.is_empty() && instance.len() <= 256 && instance.bytes().all(|c| c.is_ascii_graphic() && !matches!(c, b'/' | b'?' | b'#' | b'\\' | b'%'));
    (world_ok && instance_ok).then_some((world, instance))
}

/// VRChat's invite to oneself to `location`, one of `known` (the instances VRChat's logs listed), through `client`: the
/// instance looked at, then the invite, each step's refusal in words. Rate limited: said, nothing more is sent.
/// Each request goes only while the session is `account`'s, checked on the same hold that sends it.
pub fn invite_self(client: &Mutex<Client>, account: &str, known: &[InstanceRow], location: &str) -> InviteAnswer {
    let failed = |message: String| InviteAnswer::Failed { message };
    let changed = || failed(tr("Your VRChat account changed meanwhile: nothing was sent.").into());
    let Some((world, instance)) = location_parts(location).filter(|_| known.iter().any(|row| row.location == location)) else {
        return failed(tr("Only an instance of your Previous instances can be invited to from here.").into());
    };
    // Each request on its own hold of the session: a sign-out meanwhile stops the next.
    let looked = {
        let mut client = client.lock().unwrap();
        if client.signed_in_user_id() != Some(account) {
            return changed();
        }
        client.world_instance(world, instance)
    };
    match looked {
        Ok(Some(i)) if i.closed => return failed(tr("This instance has closed: VRChat cannot invite you there.").into()),
        Ok(Some(i)) if i.full => return failed(tr("This instance is full right now: try later or pick another.").into()),
        Ok(Some(_)) => {}
        Ok(None) | Err(Error::NotFound) => return failed(tr("This instance has closed, or VRChat does not show it to your account.").into()),
        Err(e) => return refusal(e),
    }
    let sent = {
        let mut client = client.lock().unwrap();
        if client.signed_in_user_id() != Some(account) {
            return changed();
        }
        client.invite_self(world, instance)
    };
    match sent {
        Ok(()) => InviteAnswer::Sent,
        // Seen a moment ago, refused now: the account may not go there (invite only, someone else's friends).
        Err(Error::NotFound) => failed(tr("VRChat does not let your account into this instance (invite only, or for its owner's friends).").into()),
        Err(e) => refusal(e),
    }
}

/// What VRChat's refusal means for Invite me: an expired session asks to sign in again, the rest says why.
fn refusal(e: Error) -> InviteAnswer {
    match e {
        Error::Unauthorized | Error::Disabled => InviteAnswer::NeedsVrchat { message: e.to_string() },
        e => InviteAnswer::Failed { message: e.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::Controller;

    const WORLD: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
    const ID: &str = "48213~hidden(usr_test)~region(eu)";

    fn location() -> String {
        format!("{WORLD}:{ID}")
    }

    fn known() -> Vec<InstanceRow> {
        vec![InstanceRow { location: location(), world: "Sample Lounge".into(), at: 100 }]
    }

    fn instance() -> serde_json::Value {
        serde_json::json!({"worldId": WORLD, "instanceId": ID, "accessType": "friendsPlus", "region": "eu", "n_users": 3, "capacity": 20})
    }

    /// The answer to Invite me with these replies of a stand-in VRChat, and the requests it got.
    fn asked(replies: Vec<(u16, serde_json::Value)>, location: &str) -> (InviteAnswer, Vec<(String, String)>) {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let (client, calls) = ctl.test_vrchat(replies);
        let answer = invite_self(&client, "usr_test", &known(), location);
        let calls = calls.lock().unwrap().clone();
        (answer, calls)
    }

    #[test]
    fn sent_from_the_persons_own_account_to_the_exact_instance() {
        let (answer, calls) = asked(vec![(200, instance()), (200, serde_json::json!({}))], &location());
        assert_eq!(answer, InviteAnswer::Sent);
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0 == "GET" && calls[0].1.ends_with(&format!("/instances/{WORLD}:{}", "48213~hidden(usr_test)~region(eu)")), "{calls:?}");
        assert_eq!(calls[1].0, "POST");
        assert!(calls[1].1.ends_with(&format!("/invite/myself/to/{WORLD}:{ID}")), "{calls:?}");
    }

    /// Verify3 B5: another VRChat account by the time it goes: nothing is asked of VRChat at all.
    #[test]
    fn another_vrchat_account_by_then_sends_nothing() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let (client, calls) = ctl.test_vrchat(vec![(200, instance()), (200, serde_json::json!({}))]);
        let answer = invite_self(&client, "usr_someone_else", &known(), &location());
        assert!(matches!(&answer, InviteAnswer::Failed { message } if message.contains("changed")), "{answer:?}");
        assert!(calls.lock().unwrap().is_empty(), "{:?}", calls.lock().unwrap());
    }

    #[test]
    fn closed_full_barred_rate_limited_and_expired_each_say_so_and_send_nothing_more() {
        let full = { let mut i = instance(); i["full"] = true.into(); i };
        let closed = { let mut i = instance(); i["hardClose"] = true.into(); i };
        let failed = |a: &InviteAnswer, words: &str| matches!(a, InviteAnswer::Failed { message } if message.contains(words));
        let (a, calls) = asked(vec![(200, closed)], &location());
        assert!(failed(&a, "has closed") && calls.len() == 1, "{a:?}");
        let (a, calls) = asked(vec![(200, full)], &location());
        assert!(failed(&a, "is full") && calls.len() == 1, "{a:?}");
        let (a, calls) = asked(vec![(404, serde_json::json!({"error": {"message": "not found"}}))], &location());
        assert!(failed(&a, "closed, or VRChat does not show it") && calls.len() == 1, "{a:?}");
        // Seen, then refused: not allowed in.
        let (a, calls) = asked(vec![(200, instance()), (403, serde_json::json!({"error": {"message": "nope"}}))], &location());
        assert!(failed(&a, "does not let your account into this instance") && calls.len() == 2, "{a:?}");
        let (a, calls) = asked(vec![(429, serde_json::json!({}))], &location());
        assert!(failed(&a, "rate limiting") && calls.len() == 1, "{a:?}");
        let (a, _) = asked(vec![(200, instance()), (429, serde_json::json!({}))], &location());
        assert!(failed(&a, "rate limiting"), "{a:?}");
        // The session expired: sign in again, with the way there.
        // (r20: a 401 is looked into once, who is signed in, before the session counts as ended.)
        let (a, _) = asked(vec![(401, serde_json::json!({})), (401, serde_json::json!({}))], &location());
        assert!(matches!(&a, InviteAnswer::NeedsVrchat { message } if message.contains("expired")), "{a:?}");
    }

    #[test]
    fn only_a_previous_instance_with_a_plain_location_is_asked_for() {
        for bad in [format!("{WORLD}:1~region(eu)"), "wrld_../x:1".into(), format!("{WORLD}:"), format!("{WORLD}:1/../../auth"), "usr_x:1".into(), String::new()] {
            let (a, calls) = asked(vec![(200, instance()), (200, serde_json::json!({}))], &bad);
            assert!(matches!(a, InviteAnswer::Failed { .. }) && calls.is_empty(), "{bad}: {a:?} {calls:?}");
        }
        assert_eq!(location_parts(&location()), Some((WORLD, ID)));
    }

    #[test]
    fn without_a_vrchat_session_nothing_is_sent_here() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        assert_eq!(ctl.previous_instance_invite(&location(), "usr_test"), None, "N4N0 then asks Orbiters' account");
    }
}
