//! Whose N4N0 is (verify2-r21 M1): the Orbiters **identity** signed in, and its **epoch**. The identity is the environment
//! and the member, never the credential: a token refreshed, the file read again, a reconnection, Checking becoming Signed
//! in or a restart of the service keep it, and N4N0's conversation with it. The first sign-in, a sign-out, another member
//! and another environment change it: the epoch moves by one, in the step that installs the next account and while nothing
//! of N4N0 acts, and N4N0 forgets what was the last one's.
//!
//! [`Epochs::becomes`] is the one place that decides; every account is installed through
//! [`Controller::install_account`], which asks it. The epoch is kept in a file, so that one given before a restart is
//! never given to another member after it (a window left open stamps what it sends with the epoch it saw).

use super::Controller;
use orbiters_core::account::{Account, Api, Server};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Who is signed in to Orbiters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Identity {
    /// Nobody, whatever the environment.
    #[default]
    Anonymous,
    /// A member of an environment, by the id Orbiters gives them, and their name (all an older account file has).
    Member { development: bool, user: Option<String>, name: Option<String> },
}

impl Identity {
    pub(super) fn of(server: Server, account: Option<&Account>) -> Self {
        let said = |t: &Option<String>| t.as_ref().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        match account {
            None => Identity::Anonymous,
            Some(a) => Identity::Member { development: server == Server::Development, user: said(&a.user), name: said(&a.username).map(|n| n.to_lowercase()) },
        }
    }

    /// Whether they are the same: by their ids; by their names while one of them has no id yet. Neither: another.
    fn same(&self, next: &Identity) -> bool {
        match (self, next) {
            (Identity::Anonymous, Identity::Anonymous) => true,
            (Identity::Member { development: d, user: u, name: n }, Identity::Member { development: d2, user: u2, name: n2 }) => {
                d == d2
                    && match (u, u2) {
                        (Some(a), Some(b)) => a == b,
                        _ => n.is_some() && n == n2,
                    }
            }
            _ => false,
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
}

impl Default for Epochs {
    fn default() -> Self {
        Self { epoch: orbiters_n4n0::FIRST_EPOCH, identity: Identity::Anonymous, file: None }
    }
}

impl Epochs {
    /// As they were when the service last ran: the same member signed in again goes on with their epoch.
    pub(super) fn load(file: &Path) -> Self {
        let file = Some(file.to_path_buf());
        let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok()) else { return Self { file, ..Self::default() } };
        match serde_json::from_slice::<Kept>(&bytes).ok().filter(|k| k.epoch >= orbiters_n4n0::FIRST_EPOCH) {
            Some(k) => Self { epoch: k.epoch, identity: k.identity, file },
            // Not readable: which epochs were given is not known, and none of them is given again.
            None => Self { epoch: orbiters_n4n0::unix_ms().max(orbiters_n4n0::FIRST_EPOCH), identity: Identity::Anonymous, file },
        }
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `next` is signed in now. The new epoch when that is another identity; `None` when it is the same.
    pub(super) fn becomes(&mut self, next: Identity) -> Option<u64> {
        let same = self.identity.same(&next);
        let before = std::mem::replace(&mut self.identity, next);
        if same {
            // An id known is kept when the account read now names none (an older tool wrote the file).
            if let (Identity::Member { user: Some(id), .. }, Identity::Member { user: user @ None, .. }) = (&before, &mut self.identity) {
                *user = Some(id.clone());
            }
        } else {
            self.epoch += 1;
        }
        if self.identity != before || !same {
            self.keep();
        }
        (!same).then_some(self.epoch)
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
        let (out, moved) = {
            let mut rt = self.account_runtime.lock().unwrap();
            let out = install(&mut rt);
            let moved = rt.read.map(|(server, _)| Identity::of(server, rt.account.as_ref())).and_then(|next| rt.epochs.becomes(next));
            if let Some(epoch) = moved {
                self.identity_epoch.store(epoch, Ordering::SeqCst);
            }
            (out, moved)
        };
        if let Some(epoch) = moved {
            paused.forget_account(epoch);
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

    /// [`Controller::account_api`] of identity `epoch`, read in the same step as the epoch is checked.
    pub(super) fn account_api_of(&self, epoch: u64) -> Option<(Api, String)> {
        let rt = self.account_runtime.lock().unwrap();
        if rt.epochs.epoch() != epoch {
            return None;
        }
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (Api::new(server), a.token.clone()))
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
        Identity::of(server, Some(&Account { token: "t".into(), user: user.map(str::to_string), username: name.map(str::to_string), avatar_url: None, features: None }))
    }

    /// Every pair of identities the app can hold, and whether one becoming the other moves the epoch.
    #[test]
    fn the_epoch_moves_exactly_when_the_member_changes() {
        let (prod, dev) = (Server::Production, Server::Development);
        let nobody = Identity::Anonymous;
        let rows = [
            ("nobody, still nobody", nobody.clone(), nobody.clone(), false),
            ("the first sign-in", nobody.clone(), member(prod, Some("1"), Some("a")), true),
            ("a sign-out", member(prod, Some("1"), Some("a")), nobody.clone(), true),
            ("the same member", member(prod, Some("1"), Some("a")), member(prod, Some("1"), Some("a")), false),
            ("renamed", member(prod, Some("1"), Some("a")), member(prod, Some("1"), Some("b")), false),
            ("an account file without an id, then its id", member(prod, None, Some("a")), member(prod, Some("1"), Some("A ")), false),
            ("the id, then a file without it", member(prod, Some("1"), Some("a")), member(prod, None, Some("a")), false),
            ("another member", member(prod, Some("1"), Some("a")), member(prod, Some("2"), Some("b")), true),
            ("another member of the same name", member(prod, Some("1"), Some("a")), member(prod, Some("2"), Some("a")), true),
            ("no id, another name", member(prod, None, Some("a")), member(prod, None, Some("b")), true),
            ("neither id nor name: not taken for the same", member(prod, None, None), member(prod, None, None), true),
            ("another environment, the same id", member(prod, Some("1"), Some("a")), member(dev, Some("1"), Some("a")), true),
            ("back", member(dev, Some("1"), Some("a")), member(prod, Some("1"), Some("a")), true),
        ];
        for (what, from, to, moves) in rows {
            let mut epochs = Epochs { epoch: 5, identity: from, file: None };
            assert_eq!(epochs.becomes(to), moves.then_some(6), "{what}");
            assert_eq!(epochs.epoch(), if moves { 6 } else { 5 }, "{what}");
        }
        // The id is not forgotten by a file without it: the next one's is compared with it.
        let mut epochs = Epochs { epoch: 5, identity: member(prod, Some("1"), Some("a")), file: None };
        assert_eq!(epochs.becomes(member(prod, None, Some("a"))), None);
        assert_eq!(epochs.becomes(member(prod, Some("2"), Some("a"))), Some(6));
    }

    /// A restart of the service: the same member goes on with the same epoch; another one never gets an epoch given
    /// before; an unreadable record starts over.
    #[test]
    fn a_restart_keeps_the_members_epoch_and_gives_it_to_nobody_else() {
        let dir = std::env::temp_dir().join(format!("orbiters-identity-{}-{}", std::process::id(), orbiters_n4n0::next_input()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity.json");
        let prod = Server::Production;
        let mut first = Epochs::load(&file);
        assert_eq!(first.epoch(), orbiters_n4n0::FIRST_EPOCH);
        assert_eq!(first.becomes(member(prod, Some("1"), Some("a"))), Some(2));
        let mut again = Epochs::load(&file);
        assert_eq!((again.epoch(), again.becomes(member(prod, Some("1"), Some("a")))), (2, None), "the same member");
        let mut other = Epochs::load(&file);
        assert_eq!(other.becomes(member(prod, Some("2"), Some("b"))), Some(3), "never A's 2");
        let mut gone = Epochs::load(&file);
        assert_eq!(gone.becomes(Identity::Anonymous), Some(4));
        std::fs::write(&file, b"not a record").unwrap();
        assert!(Epochs::load(&file).epoch() > 1_000_000, "none of those given before");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
