//! Whose N4N0 is (verify2-r21 M1): the Orbiters **identity** signed in, and its **epoch**. The identity is the environment
//! and the member's id, never the credential and never a name: a token refreshed, the file read again, a reconnection,
//! Checking becoming Signed in or a restart of the service keep it, and N4N0's conversation with it. The first sign-in, a
//! sign-out, another member and another environment change it: the epoch moves by one, in the step that installs the next
//! account and while nothing of N4N0 acts, and N4N0 forgets what was the last one's.
//!
//! A credential whose member is not known yet, an account file that cannot be read and an environment being switched to
//! are **unknown** (verify3-r21 B1, B2): an identity of their own, entered and left like another member's, and no one's
//! conversation: the assistant waits for it to be known.
//!
//! [`Epochs::becomes`] is the one place that decides; every account is installed through
//! [`Controller::install_account`], which asks it. The epoch is kept in a file, so that one given before a restart is
//! never given to another member after it (a window left open stamps what it sends with the epoch it saw).

use super::Controller;
use orbiters_core::account::{Account, Api, Server};
use serde::{Deserialize, Serialize};
use orbiters_n4n0::status::SignedAs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Who is signed in to Orbiters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Identity {
    /// Nobody, whatever the environment.
    #[default]
    Anonymous,
    /// Not known: a credential Orbiters named no member of yet (a digest of it; never written down), or nothing that could
    /// be read.
    Unknown {
        #[serde(skip)]
        key: Option<String>,
    },
    /// A member of an environment, by the id Orbiters gives them.
    Member { development: bool, user: String },
}

impl Identity {
    pub(super) fn of(server: Server, account: Option<&Account>) -> Self {
        let Some(account) = account else { return Identity::Anonymous };
        match account.user.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            Some(user) => Identity::Member { development: server == Server::Development, user: user.to_string() },
            None => Identity::Unknown { key: Some(format!("{}:{}", server.api(), orbiters_core::vrchat_accounts::friend_hash(&account.token))) },
        }
    }

    pub(super) fn signed_as(&self) -> SignedAs {
        match self {
            Identity::Anonymous => SignedAs::Nobody,
            Identity::Unknown { .. } => SignedAs::Unknown,
            Identity::Member { .. } => SignedAs::Member,
        }
    }
}

/// The identity signed in and its epoch, as the file keeps them.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    epoch: u64,
    identity: Identity,
}

/// The identity signed in, its epoch, and where they are kept (`None`: nowhere, tests).
#[derive(Debug)]
pub(super) struct Epochs {
    epoch: u64,
    identity: Identity,
    file: Option<PathBuf>,
    /// The member is the one the file named at the start, not signed in again yet: their conversation goes on once they are.
    resumes: bool,
}

impl Default for Epochs {
    fn default() -> Self {
        Self { epoch: orbiters_n4n0::FIRST_EPOCH, identity: Identity::Anonymous, file: None, resumes: false }
    }
}

/// What an account installed did to the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Moved {
    /// The same identity goes on.
    Same,
    /// The same member as before the service's restart, signed in again: what was kept of theirs goes on.
    Resumed,
    /// Another identity: its epoch.
    To(u64),
}

impl Epochs {
    /// As they were when the service last ran: the same member signed in again goes on with their epoch.
    pub(super) fn load(file: &Path) -> Self {
        let file = Some(file.to_path_buf());
        let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok()) else { return Self { file, ..Self::default() } };
        match serde_json::from_slice::<Kept>(&bytes).ok().filter(|k| k.epoch >= orbiters_n4n0::FIRST_EPOCH) {
            // Who it was is not known, so whoever comes is another: an unknown one is never the same as itself after a restart.
            Some(Kept { epoch, identity: Identity::Unknown { .. } }) => {
                Self { epoch, identity: Identity::Unknown { key: Some("before the restart".into()) }, file, resumes: false } // i18n: not shown
            }
            Some(k) => Self { epoch: k.epoch, resumes: matches!(k.identity, Identity::Member { .. }), identity: k.identity, file },
            // Not readable: which epochs were given is not known, and none of them is given again.
            None => Self { epoch: orbiters_n4n0::unix_ms().max(orbiters_n4n0::FIRST_EPOCH), identity: Identity::Anonymous, file, resumes: false },
        }
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `next` is signed in now.
    pub(super) fn becomes(&mut self, next: Identity) -> Moved {
        let resumes = std::mem::take(&mut self.resumes);
        if self.identity == next {
            return if resumes { Moved::Resumed } else { Moved::Same };
        }
        self.identity = next;
        self.epoch += 1;
        self.keep();
        Moved::To(self.epoch)
    }

    fn keep(&self) {
        let Some(file) = &self.file else { return };
        let kept = Kept { epoch: self.epoch, identity: self.identity.clone() };
        let written = serde_json::to_vec(&kept).map_err(anyhow::Error::from).and_then(|bytes| {
            let part = file.with_extension("part");
            std::fs::write(&part, bytes)?;
            std::fs::rename(&part, file)?;
            Ok(())
        });
        if let Err(e) = written {
            eprintln!("identity epoch: {e:#}");
        }
    }
}

impl Controller {
    /// The one way an account (or nobody) is installed in the service: `install` puts it in, once every side effect of
    /// N4N0's under way ended and while no other starts; when that is another identity, its epoch moves in the same step
    /// and N4N0 forgets the last one's conversation, what it asked, its turn under way and what is being said to it, before
    /// anything acts again. What `install` gives back.
    pub(super) fn install_account<R>(&self, install: impl FnOnce(&mut super::account::AccountRuntime) -> R) -> R {
        // Before the account's lock: what acts holds this and asks for the account (orbiters_n4n0's `fence`).
        let paused = self.n4n0.pause_effects();
        let (out, moved, epoch, known) = {
            let mut rt = self.account_runtime.lock().unwrap();
            let before = rt.signed_as();
            let out = install(&mut rt);
            let moved = rt.identity().map_or(Moved::Same, |next| rt.epochs.becomes(next));
            if let Moved::To(epoch) = moved {
                self.identity_epoch.store(epoch, Ordering::SeqCst);
            }
            (out, moved, rt.epochs.epoch(), before != rt.signed_as())
        };
        match moved {
            Moved::Same if known => self.n4n0.publish(),
            Moved::Same => {}
            Moved::Resumed => paused.resume_account(epoch),
            Moved::To(epoch) => paused.forget_account(epoch),
        }
        out
    }

    /// The epoch of the identity signed in (never 0).
    pub fn identity_epoch(&self) -> u64 {
        self.identity_epoch.load(Ordering::SeqCst)
    }

    /// The identity's epoch goes on from where the service last left it (`file`): before anything shows it.
    pub(super) fn load_identity(&self, file: &Path) {
        let mut rt = self.account_runtime.lock().unwrap();
        rt.epochs = Epochs::load(file);
        self.identity_epoch.store(rt.epochs.epoch(), Ordering::SeqCst);
    }

    /// Who is signed in, as far as it is known now.
    pub(super) fn signed_as(&self) -> SignedAs {
        self.account_runtime.lock().unwrap().signed_as()
    }

    /// [`Controller::account_api`] of the member of `epoch`, read in the same step as the epoch is checked; `None` for
    /// nobody and while who it is is not known.
    pub(super) fn account_api_of(&self, epoch: u64) -> Option<(Api, String)> {
        self.member_of(epoch).map(|(server, account)| (Api::new(server), account.token))
    }

    /// The server and the account of the member of `epoch`, read in the same step as the epoch is checked.
    pub(super) fn member_of(&self, epoch: u64) -> Option<(Server, Account)> {
        let rt = self.account_runtime.lock().unwrap();
        if rt.epochs.epoch() != epoch || rt.identity().is_none_or(|i| i.signed_as() != SignedAs::Member) {
            return None;
        }
        let (server, _) = rt.read?;
        rt.account.clone().map(|a| (server, a))
    }
}

#[cfg(test)]
impl Controller {
    /// Tests: member `user` is signed in, as the account thread installs them.
    pub(crate) fn sign_in_for_test(&self, user: &str) {
        self.install_account(|rt| {
            rt.read = Some((Server::current(), None));
            rt.account = Some(Account { token: format!("orbit-test-{user}"), user: Some(user.into()), username: Some(user.into()), avatar_url: None, features: None });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(server: Server, user: Option<&str>, name: Option<&str>) -> Identity {
        credential(server, "t", user, name)
    }

    fn credential(server: Server, token: &str, user: Option<&str>, name: Option<&str>) -> Identity {
        Identity::of(server, Some(&Account { token: token.into(), user: user.map(str::to_string), username: name.map(str::to_string), avatar_url: None, features: None }))
    }

    /// Every pair of identities the app can hold, and whether one becoming the other moves the epoch.
    #[test]
    fn the_epoch_moves_exactly_when_the_member_changes() {
        let (prod, dev) = (Server::Production, Server::Development);
        let nobody = Identity::Anonymous;
        let unread = Identity::Unknown { key: None };
        let unnamed = |token: &str| credential(prod, token, None, Some("a"));
        let rows = [
            ("nobody, still nobody", nobody.clone(), nobody.clone(), false),
            ("the first sign-in", nobody.clone(), member(prod, Some("1"), Some("a")), true),
            ("a sign-out", member(prod, Some("1"), Some("a")), nobody.clone(), true),
            ("the same member", member(prod, Some("1"), Some("a")), member(prod, Some("1"), Some("a")), false),
            ("the same member, another credential", credential(prod, "t", Some("1"), None), credential(prod, "t2", Some("1"), None), false),
            ("renamed", member(prod, Some("1"), Some("a")), member(prod, Some("1"), Some("b")), false),
            ("a credential without an id, then its id", unnamed("t"), member(prod, Some("1"), Some("a")), true),
            ("the id, then a credential without one, the same name", member(prod, Some("1"), Some("a")), unnamed("t"), true),
            ("a blank id is none", member(prod, Some("1"), Some("a")), member(prod, Some(" "), Some("a")), true),
            ("the same credential without an id, read again", unnamed("t"), unnamed("t"), false),
            ("another credential without an id, the same name", unnamed("t"), unnamed("t2"), true),
            ("the same credential without an id, another environment", unnamed("t"), credential(dev, "t", None, Some("a")), true),
            ("the file cannot be read", member(prod, Some("1"), Some("a")), unread.clone(), true),
            ("it still cannot", unread.clone(), unread.clone(), false),
            ("it can again", unread.clone(), member(prod, Some("1"), Some("a")), true),
            ("it could not, and nobody is there", unread.clone(), nobody.clone(), true),
            ("another member", member(prod, Some("1"), Some("a")), member(prod, Some("2"), Some("b")), true),
            ("another member of the same name", member(prod, Some("1"), Some("a")), member(prod, Some("2"), Some("a")), true),
            ("another environment, the same id", member(prod, Some("1"), Some("a")), member(dev, Some("1"), Some("a")), true),
            ("back", member(dev, Some("1"), Some("a")), member(prod, Some("1"), Some("a")), true),
        ];
        for (what, from, to, moves) in rows {
            let mut epochs = Epochs { epoch: 5, identity: from, ..Epochs::default() };
            assert_eq!(epochs.becomes(to), if moves { Moved::To(6) } else { Moved::Same }, "{what}");
            assert_eq!(epochs.epoch(), if moves { 6 } else { 5 }, "{what}");
        }
    }

    /// A restart of the service: the same member goes on with the same epoch (and what was kept of theirs, once); another
    /// one never gets an epoch given before; an unreadable record starts over; an unknown one is never the same again.
    #[test]
    fn a_restart_keeps_the_members_epoch_and_gives_it_to_nobody_else() {
        let dir = std::env::temp_dir().join(format!("orbiters-identity-{}-{}", std::process::id(), orbiters_n4n0::next_input()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity.json");
        let prod = Server::Production;
        let mut first = Epochs::load(&file);
        assert_eq!(first.epoch(), orbiters_n4n0::FIRST_EPOCH);
        assert_eq!(first.becomes(member(prod, Some("1"), Some("a"))), Moved::To(2));
        let mut again = Epochs::load(&file);
        assert_eq!((again.epoch(), again.becomes(member(prod, Some("1"), Some("a")))), (2, Moved::Resumed), "the same member");
        assert_eq!(again.becomes(member(prod, Some("1"), Some("a"))), Moved::Same, "once");
        let mut other = Epochs::load(&file);
        assert_eq!(other.becomes(member(prod, Some("2"), Some("b"))), Moved::To(3), "never A's 2");
        let mut gone = Epochs::load(&file);
        assert_eq!(gone.becomes(Identity::Anonymous), Moved::To(4));
        for unknown in [credential(prod, "t", None, Some("a")), Identity::Unknown { key: None }] {
            let mut before = Epochs::load(&file);
            let Moved::To(epoch) = before.becomes(unknown.clone()) else { panic!("an identity of its own") };
            assert!(!std::fs::read_to_string(&file).unwrap().contains("key"), "no digest of a credential is written down");
            let mut after = Epochs::load(&file);
            assert_eq!(after.becomes(unknown), Moved::To(epoch + 1), "not the same after a restart");
        }
        // A record of an older version (a member by name, no id) is of nobody known now.
        std::fs::write(&file, br#"{"epoch":9,"identity":{"kind":"member","development":false,"user":null,"name":"a"}}"#).unwrap();
        assert!(Epochs::load(&file).epoch() > 1_000_000);
        std::fs::write(&file, b"not a record").unwrap();
        assert!(Epochs::load(&file).epoch() > 1_000_000, "none of those given before");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Verify3-r21 E: in the service, "the account signed in now" and "the VRChat client now" are resolved in the places listed
    /// here, each for something that is the member's whoever that is by then (a reading, their own presence), or with the
    /// identity checked as it is sent. What a surface asked goes through `acting` (or N4N0's fence) and the epoch it was
    /// asked with.
    #[test]
    fn nothing_of_the_service_resolves_whoever_is_signed_in_by_itself() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("controller");
        // (file, what is asked, how many times, why it may).
        const MAY: &[(&str, &str, usize, &str)] = &[
            ("account.rs", "account::read_saved(", 2, "the account thread itself: what it reads is installed under the gate"),
            ("assistant.rs", "self.account_api()", 1, "whether somebody is signed in"),
            ("calls.rs", "account_api()", 5, "the calls' own identity: every request is stamped with it and checked as it goes (verify3 of calls)"),
            ("explorer.rs", "account_api()", 1, "reading the points shown"),
            ("maps.rs", "account_api()", 2, "reading My maps, kept by whose they are"),
            ("n4n0.rs", "c.account_api()", 1, "Host::account: whether somebody is signed in (tests_identity holds its uses)"),
            ("shared_view.rs", "account_api()", 3, "its own identity, stamped on every request and checked as it goes"),
            ("shares.rs", "account_api()", 1, "this computer's own presence"),
            ("shares.rs", "account::read(", 2, "Whose::saved: one read of who it is and their records, for what has no surface (the bridge, a developer's command), and a listing"),
            ("translate.rs", "account_api()", 1, "a line of the player's own, translated as it is asked"),
            ("vrchat_login.rs", "account_api()", 1, "the VRChat session's own owner, checked by the login itself"),
            ("world_feedback.rs", "account_api()", 1, "whether somebody is signed in"),
            ("previous_invite.rs", "vrchat_signed_client()", 2, "bound to the VRChat account it was accepted with, checked on the hold that sends"),
        ];
        let mut wrong = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // (The launcher's, the Where page's and the bridge's have their own owner: verify3's other findings.)
            if !name.ends_with(".rs") || name.contains("test") || name.starts_with("bridge_") || name.starts_with("launcher") || name == "identity.rs" || name == "vrchat_data.rs" {
                continue;
            }
            // (As Git wrote it here: LF or CRLF.)
            let source = std::fs::read_to_string(entry.path()).unwrap().replace("\r\n", "\n");
            let source = source.split("\n#[cfg(test)]\nmod ").next().unwrap();
            for what in ["account_api()", "account::read(", "account::read_saved(", "vrchat_signed_client()"] {
                let found = source.lines().filter(|l| !l.trim_start().starts_with("//") && l.contains(what) && !l.contains("fn ")).count();
                let may: usize = MAY.iter().filter(|(f, w, ..)| *f == name && w.contains(what)).map(|(_, _, n, _)| n).sum();
                if found > may {
                    wrong.push(format!("{name}: {what} {found} times, {may} known"));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
