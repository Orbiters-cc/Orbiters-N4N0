//! What a window asks of Orbiters as the member it shows (verify4-r21 P3). A window process never reads the saved account: its
//! requests go to the service with the Orbiters identity's epoch on show, and the service sends them with that member's
//! credential, under its identity gate, or refuses ([`crate::controller::Controller::member_request`]). A press on a page
//! that shows A is A's, or nothing, even in the moment another tool signed B in.

use crate::backend::Ctl;
use base64::Engine as _;
use orbiters_core::account::{Api, Server};
use orbiters_core::relay::{Answered, Asked, Relay};
use orbiters_n4n0::status::SignedAs;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// A request of the member on show, over the link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberRequest {
    pub development: bool,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub accept: Option<String>,
    #[serde(default, with = "bytes")]
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberAnswer {
    pub status: u16,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub retry_after: Option<String>,
    #[serde(default, with = "bytes")]
    pub body: Vec<u8>,
}

mod bytes {
    use super::*;

    pub fn serialize<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'d, D: serde::Deserializer<'d>>(d: D) -> Result<Vec<u8>, D::Error> {
        base64::engine::general_purpose::STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// This process' link to the service (a window, the overlay); none in the service itself and in tests.
static LINK: OnceLock<Ctl> = OnceLock::new();

/// Requests go through `ctl` from here on (once, as the process starts).
pub fn through(ctl: Ctl) {
    let _ = LINK.set(ctl);
}

/// Whether this process asks through the service (it is a window's, or the overlay's).
pub fn linked() -> bool {
    LINK.get().is_some()
}

/// The member on show, for requests to `server`: none while nobody is, while who it is is not known, or while what is on show
/// is not the service's any more.
#[derive(Clone)]
pub struct Member {
    ctl: Ctl,
    epoch: u64,
    server: Server,
    /// Their id on that environment.
    pub user: String,
}

/// The member of the frame on show: for what is pressed on it (taken as it is pressed).
pub fn now(server: Server) -> Option<Member> {
    Member::of(LINK.get()?.clone(), server)
}

/// The member signed in as this process last heard: for what a page reads for itself (the frame that shows them may be the
/// one being drawn). Its answers are taken in for that owner only ([`crate::pages::ForAccount`]).
pub fn reading(server: Server) -> Option<Member> {
    let ctl = LINK.get()?.clone();
    let epoch = ctl.view().n4n0.account_epoch;
    Member::at(ctl, server, epoch)
}

impl Member {
    pub(crate) fn of(ctl: Ctl, server: Server) -> Option<Self> {
        let epoch = ctl.shown_epoch();
        Self::at(ctl, server, epoch)
    }

    fn at(ctl: Ctl, server: Server, epoch: u64) -> Option<Self> {
        let snap = ctl.view();
        let shown = epoch != 0 && snap.n4n0.account_epoch == epoch && snap.n4n0.signed_as == SignedAs::Member;
        let user = snap.account.member.as_deref()?.strip_prefix(prefix(server))?.to_string();
        shown.then_some(Self { ctl, epoch, server, user })
    }

    /// Their API: every function of it as it is, with the credential it takes (any: none leaves this process).
    pub fn api(&self) -> (Api, String) {
        (Api::relayed(self.server.api(), Arc::new(self.clone())), String::from("the member on show")) // i18n: not shown
    }
}

/// How an environment's members' keys start ("production/42": [`crate::state::AccountStatus::member`]).
pub fn prefix(server: Server) -> &'static str {
    match server {
        Server::Production => "production/",
        Server::Development => "development/",
    }
}

impl Relay for Member {
    fn send(&self, asked: Asked) -> Result<Answered, String> {
        let Asked { method, path, content_type, accept, body } = asked;
        let request = MemberRequest { development: self.server == Server::Development, method, path, content_type, accept, body };
        let a = self.ctl.member_request(self.epoch, request)?;
        Ok(Answered { status: a.status, content_type: a.content_type, retry_after: a.retry_after, body: a.body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Local;
    use crate::controller::Controller;
    use orbiters_n4n0::Surface;

    /// Verify4-r21 P3 through the real service's side: the page shows A; B is signed in (another tool, the same environment)
    /// before the window drew B. A sign-up, a withdrawal, a check or a reading sent from it goes out as nobody.
    #[test]
    fn what_is_asked_from_a_page_showing_a_is_never_sent_as_b() {
        let server = Server::current();
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        ctl.sign_in_for_test("1");
        let backend: Ctl = Arc::new(Local::new(ctl.clone()));
        backend.shown(ctl.identity_epoch(), Surface::Window);
        let a = Member::of(backend.clone(), server).expect("A is on show");
        assert_eq!(a.user, "1");
        // B, of the same name or another: the window has not drawn them.
        ctl.sign_in_for_test("2");
        let (api, token) = a.api();
        for e in [
            api.invite_me(&token, "ev", "step").map(|_| ()).unwrap_err(),
            api.withdraw_invite(&token, "ev", "step").map(|_| ()).unwrap_err(),
            api.my_invites(&token, "ev").map(|_| ()).unwrap_err(),
            api.galleries(&token).map(|_| ()).unwrap_err(),
        ] {
            assert!(format!("{e:#}").contains("signed in before"), "{e:#}");
        }
        assert!(Member::of(backend.clone(), server).is_none(), "nor is anyone on show until the window drew B");
        backend.shown(ctl.identity_epoch(), Surface::Window);
        assert_eq!(Member::of(backend.clone(), server).map(|m| m.user), Some("2".into()));
        let other = if server == Server::Production { Server::Development } else { Server::Production };
        assert!(Member::of(backend, other).is_none(), "another environment's");
    }

    /// The service takes only an address of Orbiters' API, and only for the environment its member is of.
    #[test]
    fn only_the_apis_own_addresses_are_asked() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        ctl.sign_in_for_test("1");
        let request = |method: &str, path: &str, development: bool| MemberRequest { development, method: method.into(), path: path.into(), content_type: None, accept: None, body: Vec::new() };
        let development = Server::current() == Server::Development;
        for bad in [
            request("GET", "https://evil.example/", development),
            request("GET", "//evil.example/", development),
            request("GET", "/x", development),
            request("GET", "a/../../x", development),
            request("GET", "a\\b", development),
            request("CONNECT", "x", development),
            request("GET", "x", !development),
        ] {
            assert!(ctl.member_request(ctl.identity_epoch(), bad.clone()).is_err(), "{bad:?}");
        }
        assert!(ctl.member_request(ctl.identity_epoch() + 1, request("GET", "x", development)).is_err());
        assert!(ctl.member_request(0, request("GET", "x", development)).is_err());
    }
}
