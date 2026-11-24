//! Owner-gated, local VRChat session. Only the feature check goes to Orbiters.
use super::vrchat_scope::{enforce, Stamp};
pub(super) use super::vrchat_scope::Owner;
use super::{Controller, Shared};
use crate::state::AccountState;
use orbiters_core::account::{Api, Server};
use orbiters_core::features::Grant;
use orbiters_core::vrchat_api::{Client, Error, Gate, Login, Removed, Secret, Told, TwoFactor, User};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub revision: u64,
    pub allowed: bool,
    pub busy: bool,
    pub user: Option<User>,
    /// Whom the account shown was verified for (verify3-r21 B6: controller/vrchat_scope.rs, which alone writes all this).
    #[serde(default)]
    pub verified_for: Option<orbiters_core::launcher::VerifiedFor>,
    pub methods: Vec<TwoFactor>,
    pub error: Option<String>,
    pub recovery: Recovery,
    /// Downloaded with the isolated VRChat session. The UI never loads a protected image through Orbiters.
    pub picture: Option<Picture>,
    /// The avatar's or world's picture being replaced on VRChat (one at a time), then how that ended.
    #[serde(default)]
    pub upload: Option<PictureUpload>,
    /// The VRChat session is kept on this computer but waits for Orbiters to confirm that this Orbiters account may use
    /// it (the environment or account just changed, or Orbiters cannot be reached): it comes back by itself, and nothing
    /// shows as signed out meanwhile (r20).
    #[serde(default)]
    pub waiting: bool,
    /// The VRChat account of the session while it is not usable: the one waiting, or the one VRChat ended (then `error`
    /// says so and the card offers to sign in again).
    #[serde(default)]
    pub known: Option<User>,
    /// A session saved on this computer is being restored and nothing has said yet whether it works: Orbiters' account and
    /// its grant not answered, the file being read, VRChat not answered (r21). `known` is its last account when kept.
    #[serde(default)]
    pub restoring: bool,
}

/// What every view shows of the VRChat sign-in (r21): only `SignedOut` offers "Sign in to VRChat".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Nobody signed in for certain: no saved session, signed out, VRChat ended it, or not for this Orbiters account.
    SignedOut,
    /// A saved session is being restored or waits for Orbiters: quiet, with its last known account; no sign-in offered.
    Connecting,
    SignedIn,
}

impl Status {
    pub fn presence(&self) -> Presence {
        if self.user.is_some() {
            Presence::SignedIn
        } else if self.restoring || self.waiting {
            Presence::Connecting
        } else {
            Presence::SignedOut
        }
    }
    /// The account to show: the one signed in, or the one being connected (its last known name and picture).
    pub fn shown_user(&self) -> Option<&User> {
        self.user.as_ref().or(self.known.as_ref().filter(|_| self.presence() == Presence::Connecting))
    }
}
/// A picture replaced on VRChat (`Controller::vrchat_picture`): its step while it runs, then its outcome.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PictureUpload {
    /// Different for each upload the service ran, so a page takes each outcome once.
    pub serial: u64,
    /// The avatar's or world's ID.
    pub id: String,
    pub step: Option<orbiters_core::vrchat_api::Step>,
    /// Once it ended: done, or what went wrong.
    pub outcome: Option<Result<(), String>>,
}
impl PictureUpload {
    pub fn running(&self) -> bool {
        self.outcome.is_none()
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum Recovery {
    #[default]
    None,
    Resume,
    RemoveSavedSession,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// As base64 on the link: as a JSON array of numbers a 480 x 360 thumbnail was 2.2 to 2.5 MB of text (8 of them an
    /// answer), now 0.9 MB (CTL2-1, CTL-12).
    #[serde(with = "bytes_b64")]
    pub rgba: Vec<u8>,
}

/// Bytes as base64 text in JSON; read back from that or from the array of numbers older lines and files have.
pub(crate) mod bytes_b64 {
    use base64::Engine;
    use serde::Deserialize;

    pub fn serialize<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Text(String),
            Numbers(Vec<u8>),
        }
        match Either::deserialize(d)? {
            Either::Text(text) => base64::engine::general_purpose::STANDARD.decode(text).map_err(serde::de::Error::custom),
            Either::Numbers(numbers) => Ok(numbers),
        }
    }
}
fn picture(bytes: Vec<u8>) -> Option<Picture> {
    let image = image::load_from_memory(&bytes).ok()?.thumbnail(96, 96).to_rgba8();
    Some(Picture { width: image.width(), height: image.height(), rgba: image.into_raw() })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    SignIn { username: String, password: Secret },
    Verify { method: TwoFactor, code: Secret },
    SignOut,
    Cancel,
    Resume,
}
/// How often Orbiters is asked again whether this Orbiters account may use its VRChat sign-in (r20: was three hours, with
/// the session dropped and read again each time).
const ASK_EVERY: Duration = Duration::from_secs(30 * 60);
/// How long Orbiters' last yes holds while Orbiters cannot be reached (its server down, no network, a restart of it): a
/// no ends it at once, and nothing survives the process.
const PROOF_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

/// How long after a restore that got no answer it is tried again.
const RESTORE_AGAIN: Duration = Duration::from_secs(60);

fn new_grant() -> Grant {
    Grant::new(ASK_EVERY, PROOF_LIFETIME)
}

/// Where the Orbiters account stands for the VRChat sign-in.
pub(super) enum Orbiters {
    /// A credential held for the environment chosen now: signed in, being checked, or Orbiters out of reach; and whom it
    /// is for.
    Ready(Api, String, Stamp),
    /// The environment just switched and its account is not read yet, or nothing is read yet: VRChat waits.
    Changing,
    /// Nobody is signed in to Orbiters, or Orbiters refused the credential.
    Gone,
}

pub(super) struct Runtime {
    pub(super) gate: Arc<Gate>,
    client: Arc<Mutex<Client>>,
    /// The Orbiters identity (its server's address and credential) the gate is for.
    identity: Option<(String, String)>,
    /// Orbiters' answers by identity: an environment or account switched back to finds its own yes (r20).
    grants: HashMap<(String, String), Grant>,
    generation: u64,
    busy: bool,
    /// The saved session was read once in this process; from then on it lives in memory, handed from grant to grant.
    restored: bool,
    /// Whether a session is saved on this computer, looked for once (the file is not read for it).
    saved: Option<bool>,
    /// How Orbiters is asked (tests: a stand-in for each server, never the network).
    ask: Asker,
    /// The saved session could not be read back because VRChat or the network could not answer (at start, after
    /// sleep): read again from then on, by itself (r20: only "Retry" did).
    restore_again: Option<Instant>,
    /// The saved session is being read back and checked with VRChat now (r21).
    restoring_now: bool,
    /// Rung when the Orbiters account changes: the next look comes at once, not after its half second (r21).
    pub(super) bell: Arc<Bell>,
    /// Rung when a VRChat account becomes signed in: the launcher reads its list at once, not at its next look.
    pub(super) signed_bell: Arc<Bell>,
    /// The Orbiters account's state and environment the last publish saw, to ring `bell` when it changes.
    seen_account: Option<(std::mem::Discriminant<AccountState>, bool)>,
    /// Whom what the status says of a VRChat account is for (verify-r21 Major 2, verify2-r21 M2, verify3-r21 B6), and whom
    /// the list on Home is: shown to that one only ([`super::vrchat_scope::enforce`]).
    pub(super) status_for: Option<Stamp>,
    pub(super) list_for: Option<Stamp>,
    /// Whose the place on its way to a VRChat is ([`super::vrchat_scope`]'s `publish_joining`).
    pub(super) joining_for: Option<super::vrchat_scope::By>,
    /// VRChat could not be told of the last sign-out (why): said again after a cleanup that has nothing to tell it with
    /// (verify3-r21 M4), until a new sign-in.
    untold: Option<String>,
    /// The Orbiters account saved on this computer, at launch: whose the first state is, before the account thread
    /// installed it.
    pub(super) at_launch: Option<Owner>,
    /// Where the last account's name and picture are kept (tests: a file of their own; the app: its cache folder).
    pub(super) cache_path: Option<std::path::PathBuf>,
    /// Bumped at every sign-out and change of the Orbiters identity: a cache write begun before is not made (Major 3).
    cache_generation: u64,
    /// What every publish said of the sign-in (tests: no state any window could see is missed).
    #[cfg(test)]
    pub(super) presences: Vec<Presence>,
    /// What every publish paired: the Orbiters member installed and what of VRChat it showed (tests, verify3-r21 B6).
    #[cfg(test)]
    pub(super) pairs: Vec<Pair>,
    /// Run by a producer at a named point, between what it read and its writing it (tests: whatever can happen there).
    #[cfg(test)]
    pub(super) between: Option<Between>,
}

#[cfg(test)]
pub(super) type Between = Box<dyn FnMut(&Shared, &'static str) + Send>;

/// One publish: who is signed in to Orbiters, and what of VRChat goes out with it.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pair {
    pub member: Option<String>,
    pub epoch: u64,
    pub status: bool,
    pub list: bool,
    /// The status or the list says it was verified for anyone but the member (or the time) it goes out with.
    pub foreign: bool,
    pub text: String,
}

/// A wake-up for a thread that looks every so often: it looks at once when rung.
#[derive(Default)]
pub struct Bell {
    rung: Mutex<u64>,
    cond: std::sync::Condvar,
}
impl Bell {
    pub fn ring(&self) {
        *self.rung.lock().unwrap() += 1;
        self.cond.notify_all();
    }
    /// Waits until rung after `seen` (what the last wait returned) or for `at_most`; returns how often it was rung.
    pub fn wait(&self, seen: u64, at_most: Duration) -> u64 {
        let rung = self.rung.lock().unwrap();
        let (rung, _) = self.cond.wait_timeout_while(rung, at_most, |r| *r == seen).unwrap();
        *rung
    }
    pub fn rung(&self) -> u64 {
        *self.rung.lock().unwrap()
    }
}
/// Asks the Orbiters server at an API address whether the credential's member may use VRChat sign-in.
type Asker = Arc<dyn Fn(&Api, &str) -> anyhow::Result<bool> + Send + Sync>;
impl Default for Runtime {
    fn default() -> Self {
        let gate = Arc::new(Gate::default());
        // Tests never look at the person's session file.
        #[cfg(test)]
        let client = orbiters_core::vrchat_api::offline_client(gate.clone());
        #[cfg(not(test))]
        let client = Client::new(gate.clone());
        #[cfg(test)]
        let ask: Asker = Arc::new(|_: &Api, _: &str| Err(anyhow::anyhow!("tests never reach Orbiters")));
        #[cfg(not(test))]
        let ask: Asker = Arc::new(|api: &Api, token: &str| api.vrchat_login_feature(token));
        Self { client: Arc::new(Mutex::new(client)), gate, identity: None, grants: HashMap::new(), generation: 0, busy: false, restored: false, saved: None, ask, restore_again: None, restoring_now: false, bell: Arc::default(), signed_bell: Arc::default(), seen_account: None, status_for: None, list_for: None, joining_for: None, untold: None, at_launch: None, cache_path: account_cache_path(), cache_generation: 0, #[cfg(test)] presences: Vec::new(), #[cfg(test)] pairs: Vec::new(), #[cfg(test)] between: None }
    }
}
impl Runtime {
    /// A new grant, good for `lifetime`, with the session of the client before it. Never revives an earlier grant: queued
    /// requests and data handles keep their revoked client. False (nothing changed) while that client is busy: next look.
    fn grant(&mut self, lifetime: Duration) -> bool {
        let Ok(mut old) = self.client.try_lock() else { return false };
        let gate = Arc::new(Gate::for_duration(lifetime));
        let client = old.hand_over(gate.clone());
        drop(old);
        self.gate.revoke();
        self.gate = gate;
        self.client = Arc::new(Mutex::new(client));
        true
    }
    /// Stops the grant (the Orbiters account changing or gone). The session stays in memory and on disk.
    pub(super) fn suspend(&mut self) {
        self.gate.revoke();
        self.identity = None;
        self.generation += 1;
        self.busy = false;
    }
    /// Whether the identity's grant allows VRChat now.
    fn allowed_for(&self, identity: &(String, String)) -> bool {
        self.identity.as_ref() == Some(identity) && self.gate.allowed() && self.grants.get(identity).is_some_and(|g| g.allowed(Instant::now()))
    }
    fn saved(&mut self) -> bool {
        if self.saved.is_none() {
            self.saved = self.client.try_lock().ok().map(|c| c.session_saved());
        }
        self.saved.unwrap_or(false)
    }
}
impl Controller {
    /// Where the Orbiters account stands, for VRChat's sign-in: checking it or Orbiters out of reach keeps it (r20: only
    /// "signed in" did, so an Orbiters server that could not be reached signed VRChat out).
    pub(super) fn vrchat_orbiters(&self) -> Orbiters {
        let snap = self.view();
        if snap.account.development != (Server::current() == Server::Development) {
            return Orbiters::Changing;
        }
        match &snap.account.state {
            AccountState::SignedIn { .. } | AccountState::Checking { .. } | AccountState::Offline { .. } | AccountState::SigningIn => match self.vrchat_ready() {
                Some((api, token, stamp)) => Orbiters::Ready(api, token, stamp),
                // A first sign-in on its way over nobody's account; over one whose member is not known, it waits as ever.
                None if snap.account.state == AccountState::SigningIn && self.account_runtime.lock().unwrap().account.is_none() => Orbiters::Gone,
                None => Orbiters::Changing,
            },
            AccountState::Unknown => Orbiters::Changing,
            AccountState::SignedOut | AccountState::Expired { .. } => Orbiters::Gone,
        }
    }
    pub(super) fn vrchat_identity(&self) -> Option<(Api, String)> {
        match self.vrchat_orbiters() {
            Orbiters::Ready(api, token, _) => Some((api, token)),
            _ => None,
        }
    }
    fn same_vrchat_identity(&self, identity: &(String, String)) -> bool {
        self.vrchat_identity().is_some_and(|(api, token)| api.url("") == identity.0 && token == identity.1)
    }
    pub(super) fn start_vrchat_login(self: &Shared) {
        let left;
        let cached = {
            let mut rt = self.vrchat_login.lock().unwrap();
            // What a sign-out could not remove is tried again; a name kept without a session is nobody's, and is not read.
            let client = rt.client.clone();
            let clean = client.try_lock().map_or(true, |c| c.clean_signed_out());
            let saved = rt.saved();
            let gone = saved || rt.cache_path.as_deref().is_none_or(remove_account_cache);
            left = !(clean && gone);
            rt.cache_path.clone().filter(|_| saved)
        }
        .and_then(|p| read_account_cache(&p));
        if left {
            // Still not all removed: said, with the way to try again (verify3-r21 M4).
            self.forget_login(|st| {
                st.error = Some(not_removed());
                st.recovery = Recovery::RemoveSavedSession;
            });
        }
        self.vrchat_at_launch(cached);
        let weak = Arc::downgrade(self);
        let bell = self.vrchat_login.lock().unwrap().bell.clone();
        let _ = std::thread::Builder::new().name("vrchat-feature".into()).spawn(move || {
            let mut seen = bell.rung();
            loop {
                let Some(ctl) = weak.upgrade() else { break };
                ctl.vrchat_feature_tick();
                drop(ctl);
                // Half a second, or at once when the Orbiters account changes (r21: the first look after launch waited).
                seen = bell.wait(seen, Duration::from_millis(500));
            }
        });
    }
    /// Before the first look and before any window asks (r21): a session saved here is "connecting", with the last
    /// account's name and picture kept on this computer, never "signed out" while nothing has been asked yet.
    /// The name and picture are shown only to the Orbiters account and environment they were kept for (verify-r21
    /// Major 2): another member, or another environment, sees a plain "Connecting to VRChat…".
    pub(super) fn vrchat_at_launch(&self, cached: Option<(User, Option<Picture>, Option<String>)>) {
        let saved = self.vrchat_login.lock().unwrap().saved();
        if !saved {
            return;
        }
        self.forget_login(|st| st.restoring |= st.user.is_none());
        let Some(stamp) = self.vrchat_now() else { return };
        let own = cached.filter(|(_, _, kept_for)| kept_for.as_ref() == Some(&stamp.owner.member)).map(|(user, picture, _)| (user, picture));
        if let Some((user, picture)) = own {
            self.publish_login(&stamp, |st, _| {
                if st.user.is_none() {
                    st.restoring = true;
                    st.known = Some(user);
                    st.picture = picture;
                }
            });
        }
    }
    /// One look: follows the Orbiters identity, asks Orbiters when due, hands the session to each new grant, reads the
    /// saved session once, and says what the session is.
    pub(super) fn vrchat_feature_tick(self: &Shared) {
        let orbiters = self.vrchat_orbiters();
        let (account, identity) = match &orbiters {
            Orbiters::Ready(api, token, _) => (Some((api.clone(), token.clone())), Some((api.url(""), token.clone()))),
            _ => (None, None),
        };
        let stamp = match &orbiters {
            Orbiters::Ready(_, _, stamp) => Some(stamp.clone()),
            _ => None,
        };
        let (ask, asker) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            let asker = rt.ask.clone();
            if rt.identity != identity {
                rt.gate.revoke();
                rt.identity = identity.clone();
                rt.generation += 1;
                rt.cache_generation += 1;
                rt.busy = false;
            }
            match &identity {
                Some(id) => {
                    let now = Instant::now();
                    let grant = rt.grants.entry(id.clone()).or_insert_with(new_grant);
                    let (allowed, due, remaining) = (grant.allowed(now), grant.due(now), grant.remaining(now));
                    if allowed {
                        self.vrchat_keep_grant(&mut rt, remaining);
                    } else if rt.gate.allowed() {
                        // Orbiters' last yes is too old.
                        rt.gate.revoke();
                        rt.generation += 1;
                    }
                    (due, asker)
                }
                None => (false, asker),
            }
        };
        if let (true, Some((api, token)), Some(id)) = (ask, account, identity.clone()) {
            let answer = asker(&api, &token);
            if self.same_vrchat_identity(&id) {
                let mut rt = self.vrchat_login.lock().unwrap();
                if rt.identity.as_ref() == Some(&id) {
                    let now = Instant::now();
                    let grant = rt.grants.entry(id.clone()).or_insert_with(new_grant);
                    grant.answered(&answer, now);
                    let (allowed, remaining) = (grant.allowed(now), grant.remaining(now));
                    if allowed {
                        self.vrchat_keep_grant(&mut rt, remaining);
                    } else {
                        rt.gate.revoke();
                        rt.generation += 1;
                        rt.busy = false;
                    }
                }
            }
        }
        // The saved session, read once per process: from then on it is handed from grant to grant.
        let restore = identity.as_ref().and_then(|id| {
            let mut rt = self.vrchat_login.lock().unwrap();
            let later = rt.restore_again.is_some_and(|t| Instant::now() < t);
            if rt.restored || rt.busy || later || !rt.allowed_for(id) {
                return None;
            }
            // A session handed over signed in needs nothing; cookies without an account yet are asked who they are.
            let (signed_in, held) = rt.client.try_lock().ok().map(|c| (c.signed_in_user().is_some(), c.holds_session()))?;
            rt.restored = true;
            if signed_in {
                return None;
            }
            // Only a session there is shows as connecting (r21): none saved is "signed out" at once, without a spinner.
            let saved = held || rt.saved();
            rt.busy = true;
            rt.restoring_now = saved;
            rt.generation += 1;
            Some((rt.client.clone(), rt.generation, saved, rt.cache_generation))
        });
        if let (Some((client, generation, saved, cache_generation)), Some(id), Some(stamp)) = (restore, identity.as_ref(), stamp.as_ref()) {
            self.publish_login(stamp, |st, _| {
                st.busy = true;
                st.restoring = saved;
            });
            let result = {
                let mut c = client.lock().unwrap();
                let mut result = c.resume();
                if matches!(result, Ok(Some(Login::SignedIn(_)))) && !c.signed_in() {
                    result = Err(Error::Unauthorized);
                }
                result
            };
            // Its picture comes after: "signed in" shows as soon as VRChat said who it is (r21).
            let signed_in = match &result {
                Ok(Some(Login::SignedIn(u))) => Some(u.clone()),
                _ => None,
            };
            {
                let mut rt = self.vrchat_login.lock().unwrap();
                match &result {
                    Ok(Some(_)) => {
                        rt.saved = Some(true);
                        rt.restore_again = None;
                    }
                    // No answer is not an answer: the saved session is read again later (VRChat's own wait first).
                    Err(Error::Unavailable | Error::InvalidResponse) => {
                        rt.restored = false;
                        rt.restore_again = Some(Instant::now() + RESTORE_AGAIN);
                    }
                    Err(Error::RateLimited(wait)) => {
                        rt.restored = false;
                        rt.restore_again = Some(Instant::now() + RESTORE_AGAIN.max(*wait));
                    }
                    _ => {}
                }
                rt.restoring_now = false;
            }
            self.finish_vrchat(generation, id, stamp, result, None);
            if let Some(user) = signed_in {
                self.vrchat_login.lock().unwrap().signed_bell.ring();
                let portrait = if user.picture().is_empty() { None } else { client.lock().unwrap().image(user.picture()).ok().and_then(picture) };
                #[cfg(test)]
                self.test_point("portrait");
                let mut displayed = false;
                self.publish_login(stamp, |st, _| {
                    displayed = st.user.as_ref().is_some_and(|u| u.id == user.id);
                    if displayed && portrait.is_some() {
                        st.picture = portrait.clone();
                    }
                });
                // Kept only while that account is still the one shown, for the Orbiters account it is shown to, and no
                // sign-out or change of identity came while the picture was read (verify-r21 Major 3).
                let owner = stamp.owner.member.clone();
                let rt = self.vrchat_login.lock().unwrap();
                if displayed && rt.cache_generation == cache_generation {
                    if let Some(path) = rt.cache_path.as_deref() {
                        write_account_cache(path, &user, portrait.as_ref(), &owner);
                    }
                }
                drop(rt);
            }
        }
        self.settle_vrchat_status(matches!(orbiters, Orbiters::Gone));
    }
    /// The identity's grant holds: the gate goes on (pushed to the proof's end), or a new one takes the session over.
    fn vrchat_keep_grant(&self, rt: &mut Runtime, remaining: Duration) {
        if rt.gate.allowed() {
            rt.gate.extend(remaining);
        } else if rt.grant(remaining) {
            rt.generation += 1;
            rt.busy = false;
        }
    }
    /// What the session is, said in the status: usable, waiting for Orbiters, ended by VRChat, or not for this account.
    /// Never touches what a command under way will say.
    fn settle_vrchat_status(self: &Shared, orbiters_gone: bool) {
        let account = self.vrchat_ready();
        let (granted, waiting, busy, pending, generation, client, signed_bell) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            // The grant of the very account `account` names, or none.
            let granted = account.as_ref().is_some_and(|(api, token, _)| rt.allowed_for(&(api.url(""), token.clone())));
            let refused = rt.identity.as_ref().and_then(|id| rt.grants.get(id)).is_some_and(|g| g.refused());
            // A client busy with a request holds a session (a sign-in, a read).
            let held = rt.client.try_lock().map(|c| c.holds_session()).unwrap_or(true);
            let saved = held || rt.saved();
            let waiting = !granted && !orbiters_gone && !refused && saved;
            // The saved session is still to be read, is being read, or got no answer yet (r21).
            let pending = saved && (!rt.restored || rt.restoring_now || rt.restore_again.is_some());
            (granted, waiting, rt.busy, pending, rt.generation, rt.client.clone(), rt.signed_bell.clone())
        };
        let Ok(c) = client.try_lock() else { return };
        let user = c.signed_in_user();
        let ended = c.ended_user().cloned();
        drop(c);
        #[cfg(test)]
        self.test_point("settle");
        let stamp = account.map(|(_, _, stamp)| stamp);
        match (stamp, granted) {
            // Read for that member, with the grant it was read with: written for them only, while both still are.
            (Some(stamp), true) => {
                self.publish_login(&stamp, |st, rt| {
                    if rt.generation != generation {
                        return;
                    }
                    st.allowed = true;
                    st.waiting = false;
                    st.restoring = st.user.is_none() && st.methods.is_empty() && (pending || (busy && st.restoring));
                    // The runtime says whether a command or the restore is under way; one that was dropped (the grant
                    // changed under it) leaves nothing busy.
                    if busy {
                        return;
                    }
                    st.busy = false;
                    match user {
                        Some(u) => {
                            if st.user.is_none() {
                                signed_bell.ring();
                            }
                            st.user = Some(u);
                            st.known = None;
                            st.restoring = false;
                        }
                        None => {
                            // Whose session VRChat ended is said to the member who had it on show, and to no other.
                            let theirs = st.user.is_some() || st.known.is_some();
                            let ended = ended.filter(|_| theirs);
                            if st.user.take().is_some() {
                                // A data lookup found the session ended (a 401 looked into).
                                st.picture = None;
                                st.error = Some(ended_text(ended.as_ref()));
                            }
                            if ended.is_some() {
                                st.known = ended;
                            } else if st.methods.is_empty() && !st.restoring {
                                st.known = st.known.take().filter(|_| st.error.is_some());
                            }
                        }
                    }
                });
            }
            // Kept for a grant that is not there now: what this member had on show stays, and nothing else comes.
            (stamp, _) => {
                let kept = |st: &mut Status| {
                    let known = st.user.take().or(st.known.take());
                    *st = if waiting {
                        Status { revision: st.revision, waiting: true, restoring: true, known, picture: st.picture.take(), upload: st.upload.take(), verified_for: st.verified_for.take(), ..Status::default() }
                    } else {
                        Status { revision: st.revision, known, upload: st.upload.take(), verified_for: st.verified_for.take(), error: st.error.take().filter(|_| st.recovery == Recovery::RemoveSavedSession), recovery: if st.recovery == Recovery::RemoveSavedSession { Recovery::RemoveSavedSession } else { Recovery::None }, ..Status::default() }
                    };
                };
                match stamp {
                    Some(stamp) => {
                        self.publish_login(&stamp, |st, _| kept(st));
                    }
                    None => self.forget_login(kept),
                }
            }
        }
    }
    /// Starts immediately; all HTTP, disk and image decoding work runs away from the window thread.
    pub fn vrchat_command(self: &Shared, command: Command) {
        let Some((api, token, stamp)) = self.vrchat_ready() else { return };
        let identity = (api.url(""), token);
        let (client, generation) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            if !rt.allowed_for(&identity) || rt.busy {
                return;
            }
            // An IPC peer cannot choose a verification method VRChat did not request.
            if let Command::Verify { method, .. } = &command {
                if !self.view().vrchat_login.methods.contains(method) {
                    return;
                }
            }
            rt.gate.cancel_operations();
            rt.generation += 1;
            rt.busy = true;
            // The person's own sign-in or "Try again": the saved session is theirs to read now.
            rt.restored = true;
            // A sign-out (or another sign-in) makes any cache write begun before it moot.
            if matches!(command, Command::SignOut | Command::SignIn { .. } | Command::Cancel) {
                rt.cache_generation += 1;
            }
            (rt.client.clone(), rt.generation)
        };
        let cache_generation = self.vrchat_login.lock().unwrap().cache_generation;
        let owner = stamp.owner.member.clone();
        let replacing_session = matches!(&command, Command::SignOut | Command::SignIn { .. } | Command::Cancel);
        self.publish_login(&stamp, |st, _| {
            if replacing_session { st.user = None; st.picture = None; st.methods.clear(); }
            st.revision = st.revision.wrapping_add(1);
            st.busy = true;
            st.error = None;
        });
        let ctl = self.clone();
        std::thread::spawn(move || {
            let mut c = client.lock().unwrap();
            let signing_in = matches!(command, Command::SignIn { .. });
            let signing_out = matches!(command, Command::SignOut);
            let mut signed_out = None;
            let mut result = match command {
                Command::SignIn { username, password } => c.sign_in(username.trim(), &password.0).map(Some),
                Command::Verify { method, code } => c.verify(method, code.0.trim()).map(Some),
                Command::SignOut => c.sign_out().map(|out| {
                    signed_out = Some(out);
                    None
                }),
                Command::Cancel => c.cancel_login().map(|_| None),
                Command::Resume => c.resume(),
            };
            let picture = if let Ok(Some(Login::SignedIn(u))) = &result {
                if !u.picture().is_empty() {
                    c.image(u.picture()).ok().and_then(picture)
                } else {
                    None
                }
            } else {
                None
            };
            if matches!(result, Ok(Some(Login::SignedIn(_)))) && !c.signed_in() {
                result = Err(Error::Unauthorized);
            }
            // Signed out here whatever VRChat answered and whatever could be removed: the session is gone from memory and
            // never restored. What could not be done is said as what it is (verify2-r21 M6).
            let forgotten = signed_out.is_some();
            let mut removed = signed_out.as_ref().is_none_or(|out| out.removed == Removed::All);
            let mut untold = None;
            let ended = c.ended_user().cloned();
            drop(c);
            {
                let mut rt = ctl.vrchat_login.lock().unwrap();
                if let Some(out) = &signed_out {
                    // Told, it is told; with nothing left to tell it with, what was not told stays so (verify3-r21 M4).
                    match &out.told {
                        Told::Yes => rt.untold = None,
                        Told::Failed(e) => rt.untold = Some(e.to_string()),
                        Told::Nothing => {}
                    }
                    untold = rt.untold.clone();
                }
                if forgotten {
                    rt.saved = Some(false);
                    rt.cache_generation += 1;
                    if let Some(path) = rt.cache_path.clone() {
                        removed &= remove_account_cache(&path);
                    }
                } else if let Ok(Some(Login::SignedIn(u))) = &result {
                    rt.saved = Some(true);
                    rt.untold = None;
                    rt.signed_bell.ring();
                    if rt.cache_generation == cache_generation {
                        if let Some(path) = rt.cache_path.clone() {
                            write_account_cache(&path, u, picture.as_ref(), &owner);
                        }
                    }
                }
            }
            let recovery = match &result {
                Err(_) if signing_out => Recovery::RemoveSavedSession,
                _ if signing_out && !removed => Recovery::RemoveSavedSession,
                Err(Error::Unavailable | Error::RateLimited(_) | Error::Storage) if !signing_out => Recovery::Resume,
                _ => Recovery::None,
            };
            let clear_session = forgotten || matches!(result, Err(Error::Unauthorized));
            let result = match result {
                Err(Error::Unauthorized) if signing_in => Err(tr("The username or password was not accepted by VRChat.").to_string()),
                Err(Error::Unauthorized) => Err(ended_text(ended.as_ref())),
                // Not done at all (Orbiters stopped the use of it meanwhile): still signed in here, said so.
                Err(_) if signing_out => Err(tr("The saved VRChat session could not be removed. Try signing out again.").into()),
                r => {
                    let local = signed_out.as_ref().and_then(|out| match out.removed {
                        Removed::NotSuppressed => Some(tr("The saved VRChat session could not be removed from this computer and may sign in again at the next start: close what holds it, then remove it again.").to_string()),
                        _ if !removed => Some(not_removed()),
                        _ => None,
                    });
                    match (signed_out.is_some(), local, untold) {
                        (true, None, Some(e)) => Err(not_told(&e)),
                        (true, Some(local), None) => Err(local),
                        (true, Some(local), Some(e)) => Err(format!("{local} {}", tr!("VRChat could not be told either ({error}): the session may stay valid on VRChat's side until it expires.", error = e))),
                        _ => r.map_err(|e| e.to_string()),
                    }
                }
            };
            ctl.finish_vrchat_text(generation, &identity, &stamp, result, picture, clear_session, forgotten, recovery);
        });
    }
    fn finish_vrchat(self: &Shared, generation: u64, identity: &(String, String), stamp: &Stamp, result: orbiters_core::vrchat_api::Result<Option<Login>>, picture: Option<Picture>) {
        let recovery = match &result {
            Err(Error::Unavailable | Error::RateLimited(_) | Error::Storage) => Recovery::Resume,
            _ => Recovery::None,
        };
        let clear_session = matches!(result, Err(Error::Unauthorized));
        self.finish_vrchat_text(generation, identity, stamp, result.map_err(|e| e.to_string()), picture, clear_session, false, recovery);
    }
    #[allow(clippy::too_many_arguments)]
    fn finish_vrchat_text(
        self: &Shared,
        generation: u64,
        identity: &(String, String),
        stamp: &Stamp,
        result: Result<Option<Login>, String>,
        picture: Option<Picture>,
        clear_session: bool,
        signed_out: bool,
        recovery: Recovery,
    ) {
        if !self.same_vrchat_identity(identity) {
            return;
        }
        #[cfg(test)]
        self.test_point("finish");
        // Written for the member it was done for, while they are still the one signed in, with the same grant.
        self.publish_login(stamp, |status, rt| {
            if rt.generation != generation || rt.identity.as_ref() != Some(identity) || !rt.gate.allowed() {
                // Not shown here: the next look says what the session is (it was handed to the new grant).
                return;
            }
            rt.busy = false;
            status.busy = false;
            status.recovery = recovery;
            status.revision = status.revision.wrapping_add(1);
            if clear_session {
                status.restoring = false;
                status.known = status.user.take().or(status.known.take());
                status.picture = None;
                status.methods.clear();
            }
            match result {
                Ok(Some(Login::SignedIn(u))) => {
                    status.restoring = false;
                    status.user = Some(u);
                    status.known = None;
                    status.methods.clear();
                    status.error = None;
                    status.picture = picture;
                }
                Ok(Some(Login::TwoFactor(m))) => {
                    status.methods = m;
                    status.user = None;
                    status.picture = None;
                    status.error = None;
                }
                Ok(None) => {
                    status.restoring = false;
                    status.user = None;
                    status.known = None;
                    status.picture = None;
                    status.methods.clear();
                    status.error = None;
                }
                Err(e) => status.error = Some(e),
            }
            // Signed out: nobody's name stays.
            if signed_out {
                status.known = None;
                status.picture = None;
            }
        });
    }
    /// The VRChat client granted to the Orbiters member signed in now, and whom that is for.
    pub(super) fn vrchat_granted(&self) -> Option<(Arc<Mutex<Client>>, Stamp)> {
        let (api, token, stamp) = self.vrchat_ready()?;
        let rt = self.vrchat_login.lock().unwrap();
        rt.allowed_for(&(api.url(""), token)).then(|| (rt.client.clone(), stamp))
    }
    /// Who the session granted to the Orbiters member signed in now is signed in as, without waiting for the client (which is
    /// held while VRChat answers).
    pub(super) fn vrchat_signed_in_now(&self) -> Option<String> {
        let (api, token, _) = self.vrchat_ready()?;
        let rt = self.vrchat_login.lock().unwrap();
        rt.allowed_for(&(api.url(""), token)).then(|| rt.gate.signed_in_user_id()).flatten()
    }
    /// Tests: whatever `between` does, at the point `at` of a producer (nothing of it locked).
    #[cfg(test)]
    pub(super) fn test_point(self: &Shared, at: &'static str) {
        let hook = self.vrchat_login.lock().unwrap().between.take();
        if let Some(mut hook) = hook {
            hook(self, at);
            let mut rt = self.vrchat_login.lock().unwrap();
            if rt.between.is_none() {
                rt.between = Some(hook);
            }
        }
    }
    /// The signed-in session's client while the feature is allowed for this Orbiters account (the launcher's calls).
    pub(super) fn vrchat_signed_client(&self) -> Option<Arc<Mutex<Client>>> {
        let (api, token) = self.vrchat_identity()?;
        let rt = self.vrchat_login.lock().unwrap();
        rt.allowed_for(&(api.url(""), token)).then(|| rt.client.clone())
    }
    /// Call `avatar`, `friends`, `user`, `instance_alive` or `image` on a background thread.
    pub fn vrchat_source(&self) -> orbiters_core::vrchat_source::Source {
        let orbiters = self.vrchat_identity();
        let direct = orbiters.as_ref().and_then(|(api, token)| {
            let rt = self.vrchat_login.lock().unwrap();
            rt.allowed_for(&(api.url(""), token.clone())).then(|| rt.client.clone())
        });
        let snap = self.view();
        let w = &snap.vrchat.world;
        let log = w.local_player.as_ref().and_then(|me| w.players.iter().find(|(_, n)| *n == me).map(|(id, _)| id.clone()));
        let operation = self.vrchat_login.lock().unwrap().gate.operation(Instant::now() + Duration::from_secs(60));
        orbiters_core::vrchat_source::Source::scoped(direct, orbiters, log, operation)
    }
}

/// Signed out here; VRChat could not be told.
fn not_told(e: &str) -> String {
    tr!("Signed out on this computer, but VRChat could not be told ({error}): the session may stay valid on VRChat's side until it expires.", error = e)
}

/// Signed out; what was saved of it is not all gone.
fn not_removed() -> String {
    tr("Signed out, but the saved VRChat session could not be fully removed from this computer. It is not used any more; removing it is tried again at the next start.").into()
}

/// Where the Orbiters account leaves VRChat's grant.
pub(super) enum Grounds {
    Kept,
    /// The environment is switching: its account is not read yet.
    Switching,
    /// Nobody is signed in to Orbiters, or its credential was refused.
    Gone,
}

impl Runtime {
    /// The grant as the Orbiters account goes: stopped at once when nobody is signed in to Orbiters (or the credential was
    /// refused) and while the environment switches. The session itself is never dropped here: it stays in memory and on
    /// disk for the next grant. What shows of it follows ([`enforce`]).
    pub(super) fn follow_account(&mut self, account: &crate::state::AccountStatus) -> Grounds {
        // The Orbiters account changed (read, checked, signed in or out, the environment): VRChat's next look comes at once.
        let seen = (std::mem::discriminant(&account.state), account.development);
        if self.seen_account != Some(seen) {
            self.seen_account = Some(seen);
            self.bell.ring();
        }
        let switching = account.development != (Server::current() == Server::Development) || account.state == AccountState::Unknown;
        let gone = matches!(account.state, AccountState::SignedOut | AccountState::Expired { .. });
        if (switching || gone) && (self.identity.is_some() || self.gate.allowed()) {
            self.suspend();
        }
        match (gone, switching) {
            (true, _) => Grounds::Gone,
            (_, true) => Grounds::Switching,
            _ => Grounds::Kept,
        }
    }
}

/// What each publish does with VRChat's parts ([`enforce`]) and, in tests, what it says of them.
pub(super) fn follow_orbiters_and_record(s: &mut crate::state::Snapshot, rt: &mut Runtime, now: Option<&Stamp>) {
    enforce(s, rt, now);
    #[cfg(test)]
    rt.presences.push(s.vrchat_login.presence());
    #[cfg(test)]
    {
        let (login, list) = (&s.vrchat_login, &s.launcher);
        let signed = matches!(list.source, orbiters_core::launcher::Source::SignedIn { .. });
        rt.pairs.push(Pair {
            member: now.map(|n| n.owner.member.clone()),
            epoch: now.map_or(0, |n| n.epoch),
            status: login.user.is_some() || login.known.is_some() || login.picture.is_some() || login.upload.is_some(),
            list: signed || list.joining.is_some() || (list.me.is_some() || list.profile.is_some() || list.mine.is_some()) && !matches!(list.source, orbiters_core::launcher::Source::Database { .. }),
            foreign: [&login.verified_for, &list.verified_for]
                .into_iter()
                .flatten()
                .any(|v| now.is_none_or(|n| v.epoch != n.epoch || v.member != n.owner.member)),
            text: format!("user {:?} known {:?} list {:?}", login.user.as_ref().map(|u| &u.id), login.known.as_ref().map(|u| &u.id), list.source),
        });
    }
}

/// The last VRChat account signed in here, its name and picture (r21): shown while its saved session is restored at the
/// next launch. In the cache folder (removed with the app's data); never cookies; removed when the person signs out.
/// Tests keep none.
fn account_cache_path() -> Option<std::path::PathBuf> {
    (!cfg!(test)).then(|| orbiters_core::paths::cache_dir().join("vrchat-account.json"))
}

#[derive(Serialize, Deserialize)]
struct CachedAccount {
    id: String,
    name: String,
    #[serde(default)]
    picture: Option<Picture>,
    /// The Orbiters member and environment it may be shown to ([`Owner`]); none in a file of r21, another key in one of
    /// the version after (by the name): neither is shown.
    #[serde(default)]
    owner: Option<String>,
}

fn read_account_cache(path: &std::path::Path) -> Option<(User, Option<Picture>, Option<String>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let cached: CachedAccount = serde_json::from_str(&text).ok()?;
    let user: User = serde_json::from_value(serde_json::json!({"id": cached.id, "displayName": cached.name})).ok()?;
    Some((user, cached.picture, cached.owner))
}

/// Written with the runtime's lock held (the callers'), so no write crosses a sign-out.
fn write_account_cache(path: &std::path::Path, user: &User, picture: Option<&Picture>, owner: &str) {
    // A picture fetched before stays when this time none came (for the same account and owner).
    let picture = picture.cloned().or_else(|| read_account_cache(path).filter(|(u, _, o)| u.id == user.id && o.as_deref() == Some(owner)).and_then(|(_, p, _)| p));
    let cached = CachedAccount { id: user.id.clone(), name: user.display_name.clone(), picture, owner: Some(owner.to_string()) };
    if let (Some(dir), Ok(text)) = (path.parent(), serde_json::to_vec(&cached)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = orbiters_core::write_atomic(path, &text);
    }
}

/// Whether it is gone.
fn remove_account_cache(path: &std::path::Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// What a session VRChat ended says: whose it was, when known, and that signing in again brings it back.
fn ended_text(user: Option<&User>) -> String {
    match user.filter(|u| !u.display_name.is_empty()) {
        Some(u) => tr!("VRChat ended the session of {name}. Sign in again to use it here.", name = u.display_name),
        None => Error::Unauthorized.to_string(),
    }
}

#[cfg(test)]
mod picture_wire_tests {
    use super::Picture;

    /// CTL2-1: a 480 x 360 thumbnail crosses the link as base64 (under 1 MB of text, it was 2.2 MB or more as numbers),
    /// comes back the same, and a picture written the old way still reads.
    #[test]
    fn a_thumbnail_crosses_the_link_as_base64_and_the_old_form_still_reads() {
        let picture = Picture { width: 480, height: 360, rgba: (0..480 * 360 * 4).map(|i| (i % 251) as u8).collect() };
        let line = serde_json::to_string(&picture).unwrap();
        assert!(line.len() < 1_000_000, "{} bytes", line.len());
        assert_eq!(serde_json::from_str::<Picture>(&line).unwrap(), picture);
        let old = r#"{"width":1,"height":1,"rgba":[1,2,3,255]}"#;
        assert_eq!(serde_json::from_str::<Picture>(old).unwrap().rgba, vec![1, 2, 3, 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipc_debug_redacts_passwords_and_codes() {
        for command in [
            Command::SignIn { username: "tester".into(), password: Secret("very-secret-password".into()) },
            Command::Verify { method: TwoFactor::Totp, code: Secret("secret-code".into()) },
        ] {
            let debug = format!("{command:?}");
            assert!(!debug.contains("very-secret-password") && !debug.contains("secret-code"));
        }
    }
    #[test]
    fn signed_out_cannot_start_or_restore_a_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.vrchat_command(Command::SignIn { username: "test".into(), password: Secret("test".into()) });
        assert!(!c.view().vrchat_login.allowed);
        assert!(!c.vrchat_login.lock().unwrap().gate.allowed());
    }
    #[test]
    fn a_new_account_cannot_reactivate_an_old_grant_or_data_handle() {
        let mut runtime = Runtime::default();
        assert!(runtime.grant(Duration::from_secs(60)));
        let old = runtime.gate.clone();
        let old_client = runtime.client.clone();
        assert!(old.allowed());
        runtime.suspend();
        assert!(runtime.grant(Duration::from_secs(60)));
        assert!(runtime.gate.allowed());
        assert!(!old.allowed());
        assert!(matches!(old_client.lock().unwrap().avatar("avtr_test"), Err(Error::Disabled)));
    }
    #[test]
    fn publishing_orbiters_sign_out_revokes_vrchat_immediately() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let gate = {
            let mut rt = c.vrchat_login.lock().unwrap();
            assert!(rt.grant(Duration::from_secs(60)));
            rt.gate.clone()
        };
        c.update(|s| {
            s.account.state = AccountState::SignedOut;
            s.vrchat_login.allowed = true;
        });
        assert!(!gate.allowed());
        assert!(!c.view().vrchat_login.allowed);
    }

    /// A stand-in for each Orbiters server (r20): what it answers about VRChat sign-in, or nothing (down), and who was asked.
    #[derive(Default)]
    struct Servers {
        answers: Mutex<HashMap<String, bool>>,
        asked: Mutex<Vec<String>>,
    }
    fn servers(c: &Controller) -> Arc<Servers> {
        let servers = Arc::new(Servers::default());
        let seen = servers.clone();
        c.vrchat_login.lock().unwrap().ask = Arc::new(move |api: &Api, _: &str| {
            let url = api.url("");
            seen.asked.lock().unwrap().push(url.clone());
            match seen.answers.lock().unwrap().get(&url) {
                Some(yes) => Ok(*yes),
                None => Err(anyhow::anyhow!("connection refused")),
            }
        });
        servers
    }
    /// The Orbiters account `token` of `server`, as the account thread installs it, shown as `state`.
    fn orbiters_account(c: &Controller, server: Server, token: &str, state: AccountState) {
        {
            let mut rt = c.account_runtime.lock().unwrap();
            rt.read = Some((server, None));
            // (The same member on both environments; whoever else signs in is another.)
            let id = if matches!(token, "fake" | "there-token") { "1" } else { "9" };
            rt.account = Some(orbiters_core::account::Account { token: token.into(), user: Some(id.into()), username: Some("tester".into()), avatar_url: None, features: None });
        }
        c.update(|s| {
            s.account.state = state;
            s.account.development = Server::current() == Server::Development;
        });
    }
    /// The environment switch itself: for a moment the account on show is the other environment's.
    fn switching(c: &Controller) {
        c.update(|s| s.account.development = Server::current() != Server::Development);
    }
    fn other_server() -> Server {
        if Server::current() == Server::Development { Server::Production } else { Server::Development }
    }
    fn signed_in_as(c: &Controller) -> Option<String> {
        c.view().vrchat_login.user.as_ref().map(|u| u.id.clone())
    }
    fn tester() -> Option<String> {
        Some("usr_test".into())
    }

    /// r20, the owner's report: switching the environment dev -> prod -> dev, with the development server down on the
    /// way back, keeps the VRChat session: nothing signs out, nothing is read again, VRChat is not asked anything (its
    /// logout least of all), and in between it shows as waiting, never as a computer that never signed in.
    #[test]
    fn switching_environment_there_and_back_keeps_the_vrchat_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = c.test_vrchat(vec![]);
        let orbiters = servers(&c);
        let (here, there) = (Server::current(), other_server());
        let (here_url, there_url) = (Api::new(here).url(""), Api::new(there).url(""));
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        // The switch: the other environment's account is not read yet.
        switching(&c);
        let status = c.view().vrchat_login.clone();
        // verify-r21 Major 2: waiting, and no name: the other environment's account is not known to be this one.
        assert!(status.waiting && status.user.is_none() && status.known.is_none() && status.picture.is_none(), "{status:?}");
        c.vrchat_feature_tick();
        assert!(c.view().vrchat_login.waiting);
        // Its account read and being checked; its server says yes: the same session goes on.
        orbiters.answers.lock().unwrap().insert(there_url.clone(), true);
        orbiters_account(&c, there, "there-token", AccountState::Checking { username: Some("tester".into()) });
        c.vrchat_feature_tick();
        let status = c.view().vrchat_login.clone();
        assert_eq!(signed_in_as(&c), tester(), "{status:?}");
        assert!(status.allowed && !status.waiting && status.error.is_none());
        // Back, with the first environment's server down (Docker off): its account is offline, its earlier yes holds.
        switching(&c);
        c.vrchat_feature_tick();
        orbiters_account(&c, here, "fake", AccountState::Offline { username: Some("tester".into()) });
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "{:?}", c.view().vrchat_login);
        assert!(c.vrchat_signed_client().is_some_and(|client| client.lock().unwrap().signed_in()));
        // Nothing went to VRChat; only the server not asked before was asked.
        assert!(calls.lock().unwrap().is_empty(), "{:?}", calls.lock().unwrap());
        assert_eq!(*orbiters.asked.lock().unwrap(), vec![there_url]);
        assert!(!orbiters.asked.lock().unwrap().contains(&here_url));
    }

    /// r20: an Orbiters server that cannot be reached never signs VRChat out nor drops its session: an account it never
    /// answered for waits (kept, shown as waiting), and goes on as soon as the server answers.
    #[test]
    fn an_orbiters_server_out_of_reach_never_signs_vrchat_out() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = c.test_vrchat(vec![]);
        let orbiters = servers(&c);
        let there = other_server();
        c.vrchat_feature_tick();
        // The same account, Orbiters now out of reach and the yes due again: it stands.
        {
            let mut rt = c.vrchat_login.lock().unwrap();
            let id = rt.identity.clone().unwrap();
            rt.grants.get_mut(&id).unwrap().answered(&Err(anyhow::anyhow!("connection refused")), Instant::now());
        }
        orbiters_account(&c, Server::current(), "fake", AccountState::Offline { username: Some("tester".into()) });
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        // Another environment whose server is down and never answered: waiting, the session kept.
        orbiters_account(&c, there, "there-token", AccountState::Offline { username: Some("tester".into()) });
        for _ in 0..3 {
            c.vrchat_feature_tick();
        }
        let status = c.view().vrchat_login.clone();
        assert!(status.waiting && !status.allowed && status.user.is_none() && status.error.is_none(), "{status:?}");
        // verify2-r21 M2: not this account's to see before Orbiters and VRChat said so.
        assert!(status.known.is_none(), "{status:?}");
        assert!(c.vrchat_login.lock().unwrap().client.lock().unwrap().holds_session(), "kept in memory");
        // It answers: back, without signing in again.
        orbiters.answers.lock().unwrap().insert(Api::new(there).url(""), true);
        c.vrchat_login.lock().unwrap().grants.values_mut().for_each(|g| g.forget());
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "{:?}", c.view().vrchat_login);
        assert!(calls.lock().unwrap().is_empty());
    }

    /// r20: Orbiters saying no (this account may not use VRChat sign-in) ends it at once, but nothing is signed out at
    /// VRChat and the session stays for an account that may.
    #[test]
    fn orbiters_saying_no_stops_the_use_but_keeps_the_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = c.test_vrchat(vec![]);
        let orbiters = servers(&c);
        let there = other_server();
        c.vrchat_feature_tick();
        orbiters.answers.lock().unwrap().insert(Api::new(there).url(""), false);
        orbiters_account(&c, there, "someone-else", AccountState::SignedIn { username: "someone".into() });
        c.vrchat_feature_tick();
        let status = c.view().vrchat_login.clone();
        assert!(!status.allowed && !status.waiting && status.user.is_none(), "{status:?}");
        assert!(c.vrchat_signed_client().is_none());
        // The first account again: its yes stands, the same session goes on.
        orbiters_account(&c, Server::current(), "fake", AccountState::SignedIn { username: "tester".into() });
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        assert!(calls.lock().unwrap().is_empty());
    }

    /// r20: signing out of Orbiters stops VRChat at once (above); signing in again finds the same session, unasked.
    #[test]
    fn signing_out_of_orbiters_and_in_again_finds_the_same_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = c.test_vrchat(vec![]);
        servers(&c);
        c.vrchat_feature_tick();
        c.update(|s| s.account.state = AccountState::SignedOut);
        assert!(!c.view().vrchat_login.allowed && c.view().vrchat_login.user.is_none());
        c.vrchat_feature_tick();
        orbiters_account(&c, Server::current(), "fake", AccountState::SignedIn { username: "tester".into() });
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        assert!(calls.lock().unwrap().is_empty());
    }

    /// r20: one 401 from VRChat is looked into and changes nothing; a 401 confirmed shows the account's session ended
    /// with "Sign in again", never like a computer that never signed in; a 429 is not a sign-out.
    #[test]
    fn a_401_is_looked_into_and_a_429_is_not_a_sign_out() {
        let me = serde_json::json!({"id": "usr_test", "displayName": "Tester"});
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, _) = c.test_vrchat(vec![(401, serde_json::json!({})), (200, me.clone()), (200, serde_json::json!({"id": "avtr_a", "name": "A"}))]);
        c.vrchat_feature_tick();
        assert!(client.lock().unwrap().avatar("avtr_a").is_ok());
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        assert!(c.view().vrchat_login.error.is_none());
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, _) = c.test_vrchat(vec![(429, serde_json::json!({}))]);
        c.vrchat_feature_tick();
        assert!(matches!(client.lock().unwrap().avatar("avtr_a"), Err(Error::RateLimited(_))));
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, calls) = c.test_vrchat(vec![(401, serde_json::json!({})), (401, serde_json::json!({}))]);
        c.vrchat_feature_tick();
        assert_eq!(client.lock().unwrap().avatar("avtr_a"), Err(Error::Unauthorized));
        c.vrchat_feature_tick();
        let status = c.view().vrchat_login.clone();
        assert!(status.allowed && status.user.is_none() && status.error.is_some(), "{status:?}");
        assert_eq!(status.known.map(|u| u.id), tester(), "the account whose session ended");
        assert!(!calls.lock().unwrap().iter().any(|(_, url)| url.ends_with("/logout")));
    }

    /// r20: a saved session that could not be read back because VRChat did not answer (no network at start) is read
    /// again by itself, not only from "Retry"; one VRChat refused is not asked again and again.
    #[test]
    fn a_restore_without_an_answer_is_tried_again_by_itself() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let me = serde_json::json!({"id": "usr_test", "displayName": "Tester"});
        let (client, calls) = c.test_vrchat(vec![(503, serde_json::json!({})), (200, me)]);
        client.lock().unwrap().cancel_login().unwrap();
        orbiters_core::vrchat_api::set_test_cookie(&mut client.lock().unwrap(), "saved");
        c.vrchat_login.lock().unwrap().restored = false;
        c.vrchat_feature_tick();
        assert!(c.view().vrchat_login.user.is_none() && c.view().vrchat_login.recovery == Recovery::Resume);
        assert!(!c.vrchat_login.lock().unwrap().restored);
        c.vrchat_feature_tick();
        assert_eq!(calls.lock().unwrap().len(), 1, "not before its time");
        c.vrchat_login.lock().unwrap().restore_again = Some(Instant::now());
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "{:?}", c.view().vrchat_login);
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    /// r20: the session's cookie never leaves the service: not in any state handed to the windows and overlays, not in a
    /// notice, not in an error, whatever happened to the session (switches, waiting, ended).
    #[test]
    fn the_session_cookie_is_in_no_published_state_or_notice() {
        const COOKIE: &str = "r20-cookie-7f3a9c";
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, _) = c.test_vrchat(vec![(401, serde_json::json!({})), (401, serde_json::json!({}))]);
        orbiters_core::vrchat_api::set_test_cookie(&mut client.lock().unwrap(), COOKIE);
        let orbiters = servers(&c);
        fn look(c: &Controller) -> Vec<String> {
            vec![serde_json::to_string(&*c.view()).unwrap(), format!("{:?}", c.view().vrchat_login), format!("{:?}", c.take_notices())]
        }
        let mut seen = Vec::new();
        c.vrchat_feature_tick();
        seen.extend(look(&c));
        switching(&c);
        seen.extend(look(&c));
        orbiters.answers.lock().unwrap().insert(Api::new(other_server()).url(""), true);
        orbiters_account(&c, other_server(), "there-token", AccountState::SignedIn { username: "tester".into() });
        c.vrchat_feature_tick();
        seen.extend(look(&c));
        let client = c.vrchat_signed_client().unwrap();
        let error = client.lock().unwrap().avatar("avtr_a").unwrap_err();
        seen.push(format!("{error} {error:?}"));
        c.vrchat_feature_tick();
        seen.extend(look(&c));
        assert!(seen.iter().all(|text| !text.contains(COOKIE)), "the cookie leaked");
        assert!(seen.len() > 8);
    }

    /// A cache file of this test's own (tests never touch the app's cache folder).
    fn test_cache(name: &str) -> std::path::PathBuf {
        let path = crate::pages::test_root().join(format!("vrchat-account-{name}.json"));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// verify-r21 Major 2: the cached VRChat name and picture show only to the Orbiters account and environment they
    /// were kept for: another member, or a file kept before r21's fix (no owner), sees a plain "Connecting".
    #[test]
    fn a_cached_vrchat_name_shows_only_to_the_orbiters_account_it_was_kept_for() {
        let user: User = serde_json::from_value(serde_json::json!({"id": "usr_test", "displayName": "Tester"})).unwrap();
        let path = test_cache("owner");
        let as_member = |name: &str| {
            let c = launch(me, Some(true), true);
            c.update(|s| s.vrchat_login = Default::default());
            member(&c, Server::current(), "a-token", &format!("id of {name}"), name, checking(name));
            c
        };
        let tester = as_member("tester");
        write_account_cache(&path, &user, None, &owner_now(&tester).unwrap());
        tester.vrchat_at_launch(read_account_cache(&path));
        assert_eq!(tester.view().vrchat_login.shown_user().map(|u| u.display_name.clone()).as_deref(), Some("Tester"), "its own member");
        let other = as_member("someone-else");
        other.vrchat_at_launch(read_account_cache(&path));
        let status = other.view().vrchat_login.clone();
        assert_eq!(status.presence(), Presence::Connecting, "{status:?}");
        assert!(status.known.is_none() && status.picture.is_none() && status.shown_user().is_none(), "another member sees no name: {status:?}");
        // The other environment, same name: not the same account.
        let there = as_member("tester");
        there.update(|s| s.account.development = !s.account.development);
        there.vrchat_at_launch(read_account_cache(&path));
        assert!(there.view().vrchat_login.known.is_none(), "another environment sees no name");
        // A file kept before (no owner): never shown.
        std::fs::write(&path, br#"{"id":"usr_test","name":"Tester"}"#).unwrap();
        let old = as_member("tester");
        old.vrchat_at_launch(read_account_cache(&path));
        assert!(old.view().vrchat_login.known.is_none());
    }

    /// verify-r21 Major 2: switching the Orbiters account (or its environment) in memory hides the last VRChat name and
    /// picture until VRChat verifies the session for the new one.
    #[test]
    fn switching_the_orbiters_account_hides_the_last_vrchat_identity() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.test_vrchat(vec![]);
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        c.update(|s| s.vrchat_login.picture = Some(Picture { width: 1, height: 1, rgba: vec![1, 2, 3, 255] }));
        orbiters_account(&c, Server::current(), "another-token", AccountState::Checking { username: Some("someone-else".into()) });
        c.vrchat_feature_tick();
        let status = c.view().vrchat_login.clone();
        assert!(status.user.is_none() && status.known.is_none() && status.picture.is_none(), "{status:?}");
        assert!(status.shown_user().is_none());
    }

    /// verify-r21 Major 3: a sign-out with VRChat out of reach forgets the session here and its cached name and picture
    /// with it; only telling VRChat failed, and that is what it says.
    #[test]
    fn an_offline_sign_out_forgets_the_cached_identity() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = c.test_vrchat(vec![(503, serde_json::json!({}))]);
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        let path = test_cache("offline-sign-out");
        let user = c.view().vrchat_login.user.clone().unwrap();
        write_account_cache(&path, &user, None, "a member's key");
        c.vrchat_login.lock().unwrap().cache_path = Some(path.clone());
        c.vrchat_command(Command::SignOut);
        for _ in 0..400 {
            if !c.view().vrchat_login.busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(calls.lock().unwrap().iter().any(|(_, url)| url.ends_with("/logout")), "VRChat was asked");
        let status = c.view().vrchat_login.clone();
        assert!(!path.exists(), "the cached identity is gone with the session");
        assert_eq!(status.presence(), Presence::SignedOut, "{status:?}");
        assert!(status.error.as_deref().is_some_and(|e| e.contains("Signed out on this computer")), "{status:?}");
        assert!(status.known.is_none() && status.picture.is_none());
    }

    /// verify-r21 Major 3: the account's picture read after "signed in" is shown: if that account is no longer the one
    /// on show when the picture comes (the Orbiters account changed meanwhile), nothing is written to the cache.
    #[test]
    fn a_late_portrait_writes_nothing_for_an_account_no_longer_shown() {
        let me_with_picture = |_: &str, url: &str| {
            if url.ends_with("/auth/user") {
                (200, serde_json::json!({"id": "usr_test", "displayName": "Tester", "currentAvatarThumbnailImageUrl": "https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-000000000001/1/file"}))
            } else if url.contains("/file/") {
                std::thread::sleep(Duration::from_millis(400));
                (404, serde_json::json!({}))
            } else {
                (404, serde_json::json!({}))
            }
        };
        let c = launch(me_with_picture, Some(true), true);
        let path = test_cache("late-portrait");
        c.vrchat_login.lock().unwrap().cache_path = Some(path.clone());
        c.vrchat_feature_tick();
        account_read(&c);
        let ticking = {
            let c = c.clone();
            std::thread::spawn(move || c.vrchat_feature_tick())
        };
        // Signed in shown, its picture still being read: the Orbiters account changes.
        for _ in 0..400 {
            if c.view().vrchat_login.user.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(c.view().vrchat_login.user.is_some(), "signed in before the picture");
        orbiters_account(&c, Server::current(), "another-token", AccountState::SignedOut);
        ticking.join().unwrap();
        assert!(!path.exists(), "no cache written for an account no longer shown");
    }

    // --- verify2-r21 M2 and M6. ---

    /// The Orbiters member `id` named `name`, as the account thread installs it, shown as `state`.
    fn member(c: &Controller, server: Server, token: &str, id: &str, name: &str, state: AccountState) {
        c.install_account(|rt| {
            rt.read = Some((server, None));
            rt.account = Some(orbiters_core::account::Account { token: token.into(), user: Some(id.into()), username: Some(name.into()), avatar_url: None, features: None });
            rt.unknown = false;
        });
        c.update(|s| {
            s.account.state = state;
            s.account.development = Server::current() == Server::Development;
        });
    }
    fn checking(name: &str) -> AccountState {
        AccountState::Checking { username: Some(name.into()) }
    }
    /// A readable account with no member id (an older file), as the account thread installs it, shown as `state`.
    fn without_id(c: &Controller, token: &str, name: &str, state: AccountState) {
        c.install_account(|rt| {
            rt.read = Some((Server::current(), None));
            rt.account = Some(orbiters_core::account::Account { token: token.into(), user: None, username: Some(name.into()), avatar_url: None, features: None });
            rt.unknown = false;
        });
        c.update(|s| {
            s.account.state = state;
            s.account.development = Server::current() == Server::Development;
        });
    }
    /// Whose name the member `id` called `name` sees at launch from a cache kept for the member `kept_id` called `kept_name`.
    fn cache_seen_by(file: &str, kept_id: &str, kept_name: &str, id: &str, name: &str) -> Option<String> {
        let user: User = serde_json::from_value(serde_json::json!({"id": "usr_test", "displayName": "Tester"})).unwrap();
        let path = test_cache(file);
        let as_member = |id: &str, name: &str| {
            let c = launch(me, Some(true), true);
            c.update(|s| s.vrchat_login = Default::default());
            member(&c, Server::current(), "a-token", id, name, checking(name));
            c
        };
        let owner = as_member(kept_id, kept_name);
        write_account_cache(&path, &user, None, &owner_now(&owner).unwrap());
        let c = as_member(id, name);
        c.vrchat_at_launch(read_account_cache(&path));
        c.view().vrchat_login.shown_user().map(|u| u.display_name.clone())
    }

    /// verify2-r21 M2: the cache belongs to the environment and the member's id: a reused name, or two members with no
    /// name, are other members; a renamed member (or a name in another case) is the same one.
    #[test]
    fn verify2_the_cache_belongs_to_the_member_id_not_the_name() {
        assert_eq!(cache_seen_by("m2-own", "1", "tester", "1", "tester").as_deref(), Some("Tester"));
        assert_eq!(cache_seen_by("m2-reused", "1", "tester", "2", "tester"), None, "the same name, another member");
        assert_eq!(cache_seen_by("m2-nameless", "1", "", "2", ""), None, "two members without a name");
        assert_eq!(cache_seen_by("m2-renamed", "1", "tester", "1", "Renamed").as_deref(), Some("Tester"), "renamed");
        assert_eq!(cache_seen_by("m2-case", "1", "tester", "1", "TESTER").as_deref(), Some("Tester"), "another case");
    }

    /// verify2-r21 M2: the very first snapshot after a live switch (no look of the VRChat thread yet) hides who was shown.
    #[test]
    fn verify2_the_first_snapshot_after_a_live_switch_hides_the_identity_before() {
        for name in ["someone-else", "tester", ""] {
            let c = Controller::new(orbiters_core::settings::Settings::default());
            c.test_vrchat(vec![]);
            member(&c, Server::current(), "fake", "1", "tester", AccountState::SignedIn { username: "tester".into() });
            c.vrchat_feature_tick();
            assert_eq!(signed_in_as(&c), tester());
            c.update(|s| s.vrchat_login.picture = Some(Picture { width: 1, height: 1, rgba: vec![1, 2, 3, 255] }));
            // The switch, as the account thread publishes it.
            member(&c, Server::current(), "another-token", "2", name, checking(name));
            let status = c.view().vrchat_login.clone();
            assert!(status.user.is_none() && status.known.is_none() && status.picture.is_none() && status.shown_user().is_none(), "{name:?}: {status:?}");
            assert!(!status.allowed && status.presence() == Presence::Connecting, "{name:?}: {status:?}");
            assert!(c.vrchat_signed_client().is_none(), "nothing of it usable either");
            // The same member again, its check under way: shown once VRChat's session is this member's grant again.
            member(&c, Server::current(), "fake", "1", "tester", AccountState::SignedIn { username: "tester".into() });
            c.vrchat_feature_tick();
            assert_eq!(signed_in_as(&c), tester(), "{name:?}: back");
        }
    }

    /// What signing out says: its session kept in a file of the test's own, `break_it` making part of the removal fail,
    /// VRChat answering `logout`.
    fn signed_out(name: &str, logout: u16, break_it: impl FnOnce(&std::path::Path, &std::path::Path)) -> (Status, Vec<(String, String)>, std::path::PathBuf, std::path::PathBuf) {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, calls) = c.test_vrchat(vec![(logout, serde_json::json!({}))]);
        c.vrchat_feature_tick();
        let dir = crate::pages::test_root().join(format!("vrchat-sign-out-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join(orbiters_core::vrchat_api::SESSION_FILE);
        std::fs::write(&session, b"made-up").unwrap();
        std::fs::write(dir.join(orbiters_core::vrchat_api::SESSION_BACKUP), b"made-up").unwrap();
        orbiters_core::vrchat_api::set_test_store(&mut client.lock().unwrap(), &session);
        let cache = dir.join("vrchat-account.json");
        let user = c.view().vrchat_login.user.clone().unwrap();
        write_account_cache(&cache, &user, None, "a member's key");
        c.vrchat_login.lock().unwrap().cache_path = Some(cache.clone());
        break_it(&session, &cache);
        c.vrchat_command(Command::SignOut);
        for _ in 0..600 {
            if !c.view().vrchat_login.busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let calls = calls.lock().unwrap().clone();
        (c.view().vrchat_login.clone(), calls, session, cache)
    }
    /// The owner of what VRChat shows now, as the cache keeps it.
    fn owner_now(c: &Controller) -> Option<String> {
        c.vrchat_now().map(|n| n.owner.member)
    }
    fn as_folder(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        std::fs::create_dir_all(path.join("kept")).unwrap();
    }

    /// verify2-r21 M6: the three ways a sign-out ends, each said as what happened.
    #[test]
    fn verify2_signing_out_says_what_happened() {
        let mut wrong = Vec::new();
        let told = |calls: &[(String, String)]| calls.iter().any(|(_, url)| url.ends_with("/logout"));
        // Everywhere.
        let (status, calls, session, cache) = signed_out("everywhere", 200, |_, _| {});
        if status.error.is_some() || status.presence() != Presence::SignedOut || !told(&calls) || session.exists() || cache.exists() {
            wrong.push(format!("everywhere: {status:?}"));
        }
        // Here, VRChat could not be told: said, with what that means.
        let (status, _, session, cache) = signed_out("not-told", 503, |_, _| {});
        let text = status.error.clone().unwrap_or_default();
        if !text.contains("Signed out on this computer") || !text.contains("until it expires") || session.exists() || cache.exists() || status.recovery != Recovery::None {
            wrong.push(format!("not told: {status:?}"));
        }
        // The session's copy, then the session itself, then the kept name cannot be deleted: a local failure, whatever
        // VRChat says, and VRChat is told all the same.
        for (what, broken) in [("copy", 0), ("session", 1), ("cache", 2)] {
            let (status, calls, _, _) = signed_out(&format!("local-{what}"), 200, |session, cache| match broken {
                0 => as_folder(&session.with_file_name(orbiters_core::vrchat_api::SESSION_BACKUP)),
                1 => as_folder(session),
                _ => as_folder(cache),
            });
            let text = status.error.clone().unwrap_or_default();
            if !text.contains("could not be fully removed from this computer") || text.contains("VRChat could not") || status.recovery != Recovery::RemoveSavedSession || !told(&calls) || status.user.is_some() {
                wrong.push(format!("{what}: told={} {status:?}", told(&calls)));
            }
        }
        // Both: both said.
        let (status, _, _, _) = signed_out("both", 503, |session, _| as_folder(&session.with_file_name(orbiters_core::vrchat_api::SESSION_BACKUP)));
        let text = status.error.clone().unwrap_or_default();
        if !text.contains("could not be fully removed from this computer") || !text.contains("until it expires") {
            wrong.push(format!("both: {status:?}"));
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    use super::super::vrchat_scope::By;

    // --- verify3-r21 B6: no publish pairs one member's VRChat identity with another's Orbiters account. ---

    fn signed(name: &str) -> AccountState {
        AccountState::SignedIn { username: name.into() }
    }

    /// What was published of VRChat while another member than `key` (or nobody) was signed in to Orbiters.
    fn wrong_pairs(c: &Controller, key: &str) -> Vec<Pair> {
        let rt = c.vrchat_login.lock().unwrap();
        rt.pairs.iter().filter(|p| p.foreign || (p.status || p.list) && p.member.as_deref() != Some(key)).cloned().collect()
    }

    /// Member 1 of this environment, signed in to VRChat, with their list on show; the key their publishes have.
    fn first_member(replies: Vec<(u16, serde_json::Value)>) -> (Shared, String) {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.test_vrchat(replies);
        member(&c, Server::current(), "fake", "1", "tester", signed("tester"));
        c.test_signed_in();
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester());
        let key = owner_now(&c).unwrap();
        c.vrchat_login.lock().unwrap().pairs.clear();
        (c, key)
    }

    fn their_list() -> orbiters_core::launcher::LauncherState {
        use orbiters_core::launcher::{Friend, LauncherState, MyProfile, Source};
        LauncherState {
            source: Source::SignedIn { user_id: "usr_test".into(), name: "Tester".into() },
            me: Some(Friend { id: "usr_test".into(), name: "Tester".into(), ..Default::default() }),
            friends: vec![Friend { id: "usr_friend".into(), name: "Their friend".into(), ..Default::default() }],
            mine: Some(MyProfile { bio: "theirs".into(), ..Default::default() }),
            ..LauncherState::default()
        }
    }

    /// At `point`, member 2 (known by name, or with no name at all) signs in to Orbiters, as the account thread publishes it.
    fn switches_at(c: &Shared, point: &'static str, name: &'static str) {
        let mut done = false;
        c.vrchat_login.lock().unwrap().between = Some(Box::new(move |c, at| {
            if at == point && !std::mem::replace(&mut done, true) {
                member(c, Server::current(), "another-token", "2", name, checking(name));
            }
        }));
    }

    /// verify3-r21 B6, its exact scenario: the status read for member 1 (granted, their VRChat user) is written after
    /// member 2 took over: it is dropped, and no publish shows it under member 2's name.
    #[test]
    fn verify3_a_late_status_is_never_published_under_another_member() {
        for name in ["someone-else", "tester", ""] {
            let (c, key) = first_member(vec![]);
            switches_at(&c, "settle", name);
            c.vrchat_feature_tick();
            assert!(owner_now(&c).unwrap() != key, "switched");
            assert_eq!(wrong_pairs(&c, &key), Vec::new(), "{name:?}");
            let status = c.view().vrchat_login.clone();
            assert!(status.user.is_none() && status.known.is_none() && !status.allowed, "{name:?}: {status:?}");
        }
    }

    /// The same for a sign-in or a restore that finishes late.
    #[test]
    fn verify3_a_late_sign_in_is_never_published_under_another_member() {
        let me = serde_json::json!({"id": "usr_test", "displayName": "Tester"});
        let (c, key) = first_member(vec![(200, me)]);
        switches_at(&c, "finish", "someone-else");
        c.vrchat_command(Command::Resume);
        for _ in 0..400 {
            if owner_now(&c).is_some_and(|k| k != key) && c.vrchat_login.lock().unwrap().between.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(60));
        c.update(|_| {});
        assert_eq!(wrong_pairs(&c, &key), Vec::new());
        assert!(c.view().vrchat_login.user.is_none());
    }

    /// The friends list, the account's own card and the picture being uploaded go with the member, in the first publish.
    #[test]
    fn verify3_the_list_and_the_card_go_with_the_member_at_once() {
        for state in [checking("someone-else"), signed("someone-else"), AccountState::SignedOut] {
            let (c, key) = first_member(vec![]);
            c.test_list(their_list());
            c.update(|s| s.vrchat_login.upload = Some(PictureUpload { serial: 1, id: "avtr_theirs".into(), step: None, outcome: None }));
            assert_eq!(c.view().launcher.friends.len(), 1, "on show");
            member(&c, Server::current(), "another-token", "2", "someone-else", state.clone());
            let view = c.view();
            assert!(view.launcher.friends.is_empty() && view.launcher.me.is_none() && view.launcher.mine.is_none(), "{state:?}: {:?}", view.launcher.source);
            assert!(view.vrchat_login.upload.is_none(), "{state:?}");
            assert_eq!(wrong_pairs(&c, &key), Vec::new(), "{state:?}");
        }
    }

    /// A read of member 1's list that ends after member 2 took over (VRChat's session handed on to them, the same account
    /// of VRChat's even) is nobody's: member 2's own is read for them.
    #[test]
    fn verify3_a_late_list_is_never_published_under_another_member() {
        let (c, key) = first_member(vec![]);
        switches_at(&c, "commit", "someone-else");
        c.test_list(their_list());
        c.update(|_| {});
        assert_eq!(wrong_pairs(&c, &key), Vec::new());
        assert!(c.view().launcher.friends.is_empty());
    }

    /// What was read before a member left is refused where it is written, also once they are back (another time of theirs):
    /// what is on show by then stays as it is.
    #[test]
    fn verify3_a_late_write_is_refused_and_leaves_what_is_shown() {
        let (c, _) = first_member(vec![]);
        let old = c.vrchat_session().unwrap().by;
        member(&c, Server::current(), "another-token", "2", "someone-else", signed("someone-else"));
        member(&c, Server::current(), "fake", "1", "tester", signed("tester"));
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "back");
        c.test_list(their_list());
        let before = c.view();
        assert_eq!(before.launcher.friends.len(), 1);
        assert!(!c.publish_login(&old.stamp, |st, _| st.known = Some(User { id: "usr_late".into(), ..Default::default() })));
        assert!(!c.publish_launcher(&old, |l| l.friends.clear()));
        assert!(!c.publish_launcher(&By { user: "usr_other".into(), ..c.vrchat_session().unwrap().by }, |l| l.friends.clear()));
        c.update(|_| {});
        let after = c.view();
        assert_eq!((&after.vrchat_login, &after.launcher), (&before.vrchat_login, &before.launcher));
        assert!(c.publish_launcher(&c.vrchat_session().unwrap().by, |l| l.friends.clear()), "their own, now");
    }

    /// While whose the account installed is is unknown (its file cannot be read, or names no member: controller/identity.rs),
    /// nothing of VRChat is anybody's: not shown, not used; read for the member again once they are known. `allowed`: what
    /// Orbiters says of the credential, asked or not.
    #[test]
    fn verify3_nothing_of_vrchat_is_anybodys_while_the_member_is_unknown() {
        type Unknown = fn(&Controller);
        let cases: [(&str, Unknown); 6] = [
            ("the file cannot be read", |c| {
                c.install_account(|rt| rt.unknown = true);
                c.update(|_| {});
            }),
            // Readable, with no member id: the very credential VRChat was granted to, then another.
            ("no id, signed in", |c| without_id(c, "fake", "tester", signed("tester"))),
            ("no id, being checked", |c| without_id(c, "fake", "tester", checking("tester"))),
            ("no id, Orbiters out of reach", |c| without_id(c, "fake", "tester", AccountState::Offline { username: Some("tester".into()) })),
            ("no id, another credential", |c| without_id(c, "an-older-file", "tester", signed("tester"))),
            // A new sign-in on its way over one with no id: it is not nobody either.
            ("no id, signing in", |c| without_id(c, "fake", "tester", AccountState::SigningIn)),
        ];
        let mut wrong = Vec::new();
        for (what, unknown) in cases {
            for allowed in [true, false] {
                let (c, key) = first_member(vec![]);
                let orbiters = servers(&c);
                orbiters.answers.lock().unwrap().insert(Api::new(Server::current()).url(""), allowed);
                c.test_list(their_list());
                unknown(&c);
                for look in 0..3 {
                    let view = c.view();
                    let st = &view.vrchat_login;
                    let held = c.vrchat_now().is_none() && c.vrchat_ready().is_none() && c.vrchat_session().is_none() && c.vrchat_signed_client().is_none();
                    if !held || st.shown_user().is_some() || st.user.is_some() || st.verified_for.is_some() || st.allowed || !view.launcher.friends.is_empty() || st.presence() != Presence::Connecting {
                        wrong.push(format!("{what}, allowed {allowed}, look {look}: held {held}, {st:?}, {:?}", view.launcher.source));
                    }
                    c.vrchat_command(Command::Resume);
                    c.vrchat_feature_tick();
                }
                if !wrong_pairs(&c, &key).is_empty() || !orbiters.asked.lock().unwrap().is_empty() {
                    wrong.push(format!("{what}, allowed {allowed}: {:?}, asked {:?}", wrong_pairs(&c, &key), orbiters.asked.lock().unwrap()));
                }
                member(&c, Server::current(), "fake", "1", "tester", signed("tester"));
                c.vrchat_feature_tick();
                if signed_in_as(&c) != tester() {
                    wrong.push(format!("{what}, allowed {allowed}: known again, {:?}", c.view().vrchat_login));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// verify4-r21 P5, at start-up: a saved account with no member id is "connecting" from the first state on, with no name
    /// and nothing asked of VRChat, whatever Orbiters would say of its credential; once the member is known, signed in.
    #[test]
    fn verify4_a_saved_account_without_an_id_is_connecting_until_its_member_is_known() {
        let (c, asked) = launch_as(None);
        let older = || orbiters_core::account::Account { user: None, ..tester_account() };
        let nothing = |c: &Shared, what: &str| {
            let st = c.view().vrchat_login.clone();
            assert_eq!(st.presence(), Presence::Connecting, "{what}: {st:?}");
            assert!(c.vrchat_now().is_none() && c.vrchat_ready().is_none() && st.shown_user().is_none() && st.verified_for.is_none() && !st.allowed, "{what}: {st:?}");
            assert!(asked.lock().unwrap().is_empty(), "{what}: nothing asked of VRChat: {:?}", asked.lock().unwrap());
        };
        nothing(&c, "the first state");
        c.vrchat_feature_tick();
        nothing(&c, "before the file is installed");
        for state in [checking("tester"), AccountState::Offline { username: Some("tester".into()) }, signed("tester")] {
            c.install_account(|rt| {
                rt.read = Some((Server::current(), None));
                rt.account = Some(older());
            });
            c.update(|s| s.account.state = state.clone());
            nothing(&c, "installed");
            c.vrchat_feature_tick();
            nothing(&c, "after a look");
        }
        // Orbiters' check names the member.
        member(&c, Server::current(), "fake", "1", "tester", signed("tester"));
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "{:?}", c.view().vrchat_login);
        assert!(c.view().vrchat_login.verified_for.is_some());
    }

    /// The random walk's numbers (a xorshift: the same each run).
    struct Dice(u64);
    impl Dice {
        fn roll(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) % n
        }
    }

    /// verify3-r21: account switches, environment switches, sign-outs of either account and back, interleaved with looks,
    /// commands and late completions of every kind (each producer is stopped between what it read and its writing it): no
    /// publish ever pairs what is member 1's of VRChat with anybody else's Orbiters account.
    #[test]
    fn verify3_no_interleaving_pairs_one_members_vrchat_with_another_account() {
        let (mut looked, mut shown) = (0usize, 0usize);
        for seed in 1..=60u64 {
            let (c, key) = first_member(Vec::new());
            let orbiters = servers(&c);
            // Orbiters allows member 1 here only: whatever anyone else sees of VRChat is member 1's.
            orbiters.answers.lock().unwrap().insert(Api::new(Server::current()).url(""), false);
            let dice = Arc::new(Mutex::new(Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)));
            let change = {
                let dice = dice.clone();
                move |c: &Shared| {
                    let roll = dice.lock().unwrap().roll(14);
                    match roll {
                        0..=2 | 10 => member(c, Server::current(), "fake", "1", "tester", signed("tester")),
                        9 => member(c, Server::current(), "another-token", "2", "tester", signed("tester")),
                        3 => member(c, Server::current(), "a-third-token", "3", "", checking("")),
                        4 => member(c, other_server(), "fake", "1", "tester", signed("tester")),
                        5 => switching(c),
                        6 => c.update(|s| s.account.state = AccountState::SignedOut),
                        // The account's file cannot be read: whose it is is unknown until it can.
                        7 => c.install_account(|rt| rt.unknown = true),
                        8 => c.account_environment_changed(),
                        // Readable, with no member id: member 1's own credential, and another.
                        11 => without_id(c, "fake", "tester", signed("tester")),
                        12 => without_id(c, "an-older-file", "tester", checking("tester")),
                        _ => {}
                    }
                }
            };
            {
                let (dice, change) = (dice.clone(), change.clone());
                c.vrchat_login.lock().unwrap().between = Some(Box::new(move |c, _| {
                    if dice.lock().unwrap().roll(2) == 0 {
                        change(c);
                    }
                }));
            }
            for _ in 0..120 {
                let roll = dice.lock().unwrap().roll(14);
                match roll {
                    0..=2 => change(&c),
                    3 | 4 => c.vrchat_feature_tick(),
                    5 => c.test_list(their_list()),
                    6 => c.test_finish(Ok(Some(Login::SignedIn(User { id: "usr_test".into(), ..Default::default() })))),
                    7 => {
                        // Signed out of VRChat and, some day, in again (a look puts the session there is back).
                        c.forget_login(|st| *st = Status { allowed: st.allowed, ..Status::default() });
                    }
                    // Read with the session there was; what else there is of its account is written some changes later.
                    8..=12 => {
                        let Some(session) = c.vrchat_session() else { continue };
                        let changes = dice.lock().unwrap().roll(3);
                        for _ in 0..changes {
                            change(&c);
                        }
                        let mine = their_list();
                        match roll {
                            8 => drop(c.publish_launcher(&session.by, |l| l.profile = Some(orbiters_core::launcher::Profile { friend: mine.friends[0].clone(), ..Default::default() }))),
                            9 => drop(c.publish_launcher(&session.by, |l| (l.me, l.mine) = (mine.me, mine.mine))),
                            12 => drop(c.publish_joining(Some(&session.by), orbiters_core::launcher::Joining::default())),
                            10 => drop(c.publish_login(&session.by.stamp, |st, _| st.upload = Some(PictureUpload { serial: 1, id: "avtr_theirs".into(), step: None, outcome: None }))),
                            _ => drop(c.publish_login(&session.by.stamp, |st, _| st.known = Some(User { id: "usr_test".into(), ..Default::default() }))),
                        }
                    }
                    _ => c.update(|_| {}),
                }
            }
            let rt = c.vrchat_login.lock().unwrap();
            looked += rt.pairs.len();
            shown += rt.pairs.iter().filter(|p| p.status || p.list).count();
            drop(rt);
            assert_eq!(wrong_pairs(&c, &key), Vec::new(), "seed {seed}");
        }
        assert!(looked > 3000 && shown > 300, "the walk went through it: {looked} published, {shown} with VRChat's");
    }

    /// verify3-r21 M4: VRChat could not be told, and part of the session was left here: cleaning that up later does not
    /// say VRChat was told.
    #[test]
    fn verify3_a_cleanup_retry_keeps_what_vrchat_was_not_told() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let (client, calls) = c.test_vrchat(vec![(503, serde_json::json!({}))]);
        c.vrchat_feature_tick();
        let dir = crate::pages::test_root().join("vrchat-sign-out-retry");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join(orbiters_core::vrchat_api::SESSION_FILE);
        std::fs::write(&session, b"made-up").unwrap();
        let copy = dir.join(orbiters_core::vrchat_api::SESSION_BACKUP);
        as_folder(&copy);
        orbiters_core::vrchat_api::set_test_store(&mut client.lock().unwrap(), &session);
        let finished = |c: &Shared| {
            c.vrchat_command(Command::SignOut);
            for _ in 0..600 {
                if !c.view().vrchat_login.busy {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            c.view().vrchat_login.error.clone().unwrap_or_default()
        };
        let first = finished(&c);
        assert!(first.contains("could not be fully removed") && first.contains("until it expires"), "{first}");
        // The copy can be removed by now: "Remove the saved session" again.
        std::fs::remove_dir_all(&copy).unwrap();
        let again = finished(&c);
        assert!(!again.contains("could not be fully removed"), "{again}");
        assert!(again.contains("until it expires"), "VRChat was never told: {again:?}");
        assert_eq!(calls.lock().unwrap().iter().filter(|(_, url)| url.ends_with("/logout")).count(), 1, "nothing left to tell it with");
    }

    /// verify3-r21 M4: what a sign-out could still not remove at the next start is said then too, with the way to try
    /// again; also once somebody is signed in to Orbiters and the first looks are over.
    #[test]
    fn a_start_that_can_still_not_remove_a_session_says_so() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.test_vrchat(vec![]);
        c.update(|s| s.vrchat_login = Status::default());
        let dir = crate::pages::test_root().join("vrchat-left-at-start");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("vrchat-account.json");
        as_folder(&cache);
        {
            let mut rt = c.vrchat_login.lock().unwrap();
            rt.client = Arc::new(Mutex::new(orbiters_core::vrchat_api::offline_client(rt.gate.clone())));
            rt.cache_path = Some(cache.clone());
            rt.saved = Some(false);
        }
        c.start_vrchat_login();
        for _ in 0..3 {
            c.vrchat_feature_tick();
            let status = c.view().vrchat_login.clone();
            assert!(status.error.as_deref().is_some_and(|e| e.contains("could not be fully removed")), "{status:?}");
            assert_eq!(status.recovery, Recovery::RemoveSavedSession);
        }
    }

    /// r21: a launch with a session saved here, as the service's `start()` makes it: the account file read (Checking),
    /// the sign-in's first state, then the looks. `vrchat` answers VRChat's requests; Orbiters says `orbiters`.
    fn launch(vrchat: impl Fn(&str, &str) -> (u16, serde_json::Value) + Send + Sync + 'static, orbiters: Option<bool>, saved: bool) -> Shared {
        launch_of(vrchat, orbiters, saved, tester_account()).0
    }
    /// [`launch`] with a session saved, of the Orbiters account with the member id `id`; and what is asked of VRChat.
    fn launch_as(id: Option<&str>) -> (Shared, Arc<Mutex<Vec<(String, String)>>>) {
        launch_of(me, Some(true), true, orbiters_core::account::Account { user: id.map(str::to_owned), ..tester_account() })
    }
    fn launch_of(vrchat: impl Fn(&str, &str) -> (u16, serde_json::Value) + Send + Sync + 'static, orbiters: Option<bool>, saved: bool, account: orbiters_core::account::Account) -> (Shared, Arc<Mutex<Vec<(String, String)>>>) {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let gate = Arc::new(Gate::default());
        let (client, asked) = orbiters_core::vrchat_api::test_server(gate.clone(), Duration::ZERO, vrchat);
        let client = if saved { client } else { orbiters_core::vrchat_api::offline_client(gate) };
        c.test_saved_vrchat(client, move |_: &Api, _: &str| orbiters.ok_or_else(|| anyhow::anyhow!("connection refused")));
        c.vrchat_login.lock().unwrap().saved = Some(saved);
        c.account_saved_at_launch(Server::current(), Some(account));
        let cached = serde_json::from_value(serde_json::json!({"id": "usr_test", "displayName": "Tester"})).unwrap();
        let owner = owner_now(&c);
        c.vrchat_at_launch(Some((cached, None, owner)));
        // From here a window can be connected (the service listens once `start()` returned).
        c.vrchat_login.lock().unwrap().presences.clear();
        c.update(|_| {});
        (c, asked)
    }
    /// The account file's credential installed (the account thread's read), still being checked by Orbiters.
    fn account_read(c: &Controller) {
        {
            let mut rt = c.account_runtime.lock().unwrap();
            rt.read = Some((Server::current(), None));
            rt.account = Some(tester_account());
        }
        c.update(|s| s.account.state = AccountState::Checking { username: Some("tester".into()) });
    }
    /// The Orbiters account saved on the computer these launches are on.
    fn tester_account() -> orbiters_core::account::Account {
        orbiters_core::account::Account { token: "fake".into(), user: Some("1".into()), username: Some("tester".into()), avatar_url: None, features: None }
    }
    fn presences(c: &Controller) -> Vec<Presence> {
        c.vrchat_login.lock().unwrap().presences.clone()
    }
    fn me(_: &str, url: &str) -> (u16, serde_json::Value) {
        if url.ends_with("/auth/user") { (200, serde_json::json!({"id": "usr_test", "displayName": "Tester"})) } else { (404, serde_json::json!({})) }
    }

    /// r21, the owner's report: with a session saved here, no state any window could see offers "Sign in to VRChat"
    /// before the outcome is known: it is "connecting" with the last account's name from the first state on, then
    /// signed in, without waiting for anything else (the friends list).
    #[test]
    fn a_saved_session_is_connecting_from_launch_until_signed_in() {
        let c = launch(me, Some(true), true);
        let status = c.view().vrchat_login.clone();
        assert_eq!(status.presence(), Presence::Connecting, "{status:?}");
        assert_eq!(status.shown_user().map(|u| u.display_name.as_str()), Some("Tester"), "its last known name at once");
        c.vrchat_feature_tick();
        account_read(&c);
        c.vrchat_feature_tick();
        assert_eq!(signed_in_as(&c), tester(), "{:?}", c.view().vrchat_login);
        let seen = presences(&c);
        let first_signed_in = seen.iter().position(|p| *p == Presence::SignedIn).expect("signed in");
        assert!(seen[..first_signed_in].iter().all(|p| *p == Presence::Connecting), "{seen:?}");
        assert!(!seen.contains(&Presence::SignedOut), "{seen:?}");
    }

    /// r21: VRChat saying the saved session is over is the one way to "signed out" from connecting, at its answer only.
    #[test]
    fn a_dead_saved_session_turns_to_sign_in_again_only_at_vrchats_answer() {
        let c = launch(|_, _| (401, serde_json::json!({})), Some(true), true);
        c.vrchat_feature_tick();
        account_read(&c);
        c.vrchat_feature_tick();
        let status = c.view().vrchat_login.clone();
        assert_eq!(status.presence(), Presence::SignedOut, "{status:?}");
        assert!(status.error.is_some() && !status.restoring);
        let seen = presences(&c);
        let first_out = seen.iter().position(|p| *p == Presence::SignedOut).unwrap();
        assert!(seen[..first_out].iter().all(|p| *p == Presence::Connecting), "{seen:?}");
        assert!(seen[first_out..].iter().all(|p| *p == Presence::SignedOut), "{seen:?}");
    }

    /// r21: nothing saved here: "signed out" at once and throughout, never a spinner.
    #[test]
    fn without_a_saved_session_it_is_signed_out_at_once_without_a_spinner() {
        let c = launch(me, Some(true), false);
        assert_eq!(c.view().vrchat_login.presence(), Presence::SignedOut);
        c.vrchat_feature_tick();
        account_read(&c);
        for _ in 0..3 {
            c.vrchat_feature_tick();
        }
        let seen = presences(&c);
        assert!(!seen.is_empty() && seen.iter().all(|p| *p == Presence::SignedOut), "{seen:?}");
        assert!(!c.view().vrchat_login.restoring);
    }

    /// r21 with r20's rule: Orbiters out of reach (or slow) keeps a saved session "connecting", never "signed out", for
    /// as long as it does not answer; eligibility is still never decided from what is on disk.
    #[test]
    fn orbiters_out_of_reach_keeps_a_saved_session_connecting() {
        let c = launch(me, None, true);
        c.vrchat_feature_tick();
        account_read(&c);
        c.update(|s| s.account.state = AccountState::Offline { username: Some("tester".into()) });
        for _ in 0..5 {
            c.vrchat_feature_tick();
        }
        let status = c.view().vrchat_login.clone();
        assert!(status.presence() == Presence::Connecting && status.user.is_none() && !status.allowed, "{status:?}");
        assert!(presences(&c).iter().all(|p| *p == Presence::Connecting), "{:?}", presences(&c));
        assert!(c.vrchat_signed_client().is_none(), "nothing usable without Orbiters' yes");
    }

    /// r21: VRChat not answering the saved session (no network) keeps it connecting, read again by itself.
    #[test]
    fn vrchat_not_answering_keeps_a_saved_session_connecting() {
        let c = launch(|_, _| (503, serde_json::json!({})), Some(true), true);
        c.vrchat_feature_tick();
        account_read(&c);
        c.vrchat_feature_tick();
        assert_eq!(c.view().vrchat_login.presence(), Presence::Connecting, "{:?}", c.view().vrchat_login);
        assert!(!presences(&c).contains(&Presence::SignedOut));
    }

    #[test]
    fn presence_says_what_every_view_shows() {
        let mut s = Status::default();
        assert_eq!(s.presence(), Presence::SignedOut);
        s.known = Some(User { id: "usr_test".into(), ..Default::default() });
        assert_eq!(s.presence(), Presence::SignedOut, "an ended session's name alone is not connecting");
        assert!(s.shown_user().is_none());
        s.restoring = true;
        assert_eq!(s.presence(), Presence::Connecting);
        assert_eq!(s.shown_user().map(|u| u.id.as_str()), Some("usr_test"));
        s.restoring = false;
        s.waiting = true;
        assert_eq!(s.presence(), Presence::Connecting);
        s.user = Some(User { id: "usr_test".into(), ..Default::default() });
        assert_eq!(s.presence(), Presence::SignedIn);
    }
}

#[cfg(test)]
impl Controller {
    /// A saved session at launch (r21 timing tests): `client` holds its cookie and nobody is signed in yet; Orbiters is
    /// asked through `ask`.
    pub(super) fn test_saved_vrchat(&self, client: Client, ask: impl Fn(&Api, &str) -> anyhow::Result<bool> + Send + Sync + 'static) {
        let mut rt = self.vrchat_login.lock().unwrap();
        rt.client = Arc::new(Mutex::new(client));
        rt.restored = false;
        rt.saved = Some(true);
        rt.ask = Arc::new(ask);
    }
    /// [`Self::test_vrchat`] over a [`orbiters_core::vrchat_api::scripted_client`]: bodies recorded, drops possible.
    pub(crate) fn test_vrchat_routed(&self, route: Arc<dyn Fn(&orbiters_core::vrchat_api::Sent) -> orbiters_core::vrchat_api::Routed + Send + Sync>) -> Arc<Mutex<Vec<orbiters_core::vrchat_api::Sent>>> {
        self.test_vrchat_with(|gate| orbiters_core::vrchat_api::routed_client(gate, route, false))
    }
    pub(crate) fn test_vrchat_routed_paced(&self, route: Arc<dyn Fn(&orbiters_core::vrchat_api::Sent) -> orbiters_core::vrchat_api::Routed + Send + Sync>) -> Arc<Mutex<Vec<orbiters_core::vrchat_api::Sent>>> {
        self.test_vrchat_with(|gate| orbiters_core::vrchat_api::routed_client(gate, route, true))
    }
    /// The waits VRChat asked the client for are over.
    pub(crate) fn groups_forget_waits_for_tests(&self) {
        let client = self.vrchat_login.lock().unwrap().client.clone();
        client.lock().unwrap().forget_waits();
    }
    pub(crate) fn test_vrchat_scripted(&self, replies: Vec<orbiters_core::vrchat_api::Scripted>) -> Arc<Mutex<Vec<orbiters_core::vrchat_api::Sent>>> {
        self.test_vrchat_with(|gate| orbiters_core::vrchat_api::scripted_client(gate, replies))
    }
    fn test_vrchat_with(&self, make: impl FnOnce(Arc<orbiters_core::vrchat_api::Gate>) -> (Client, Arc<Mutex<Vec<orbiters_core::vrchat_api::Sent>>>)) -> Arc<Mutex<Vec<orbiters_core::vrchat_api::Sent>>> {
        self.test_account_with_user("1");
        let identity = self.vrchat_identity().unwrap();
        let mut rt = self.vrchat_login.lock().unwrap();
        assert!(rt.grant(Duration::from_secs(3600)));
        let id = (identity.0.url(""), identity.1);
        rt.identity = Some(id.clone());
        rt.grants.entry(id).or_insert_with(new_grant).answered(&Ok(true), Instant::now());
        rt.restored = true;
        let (client, sent) = make(rt.gate.clone());
        rt.client = Arc::new(Mutex::new(client));
        drop(rt);
        // As a sign-in publishes it: verified for the member signed in, at this time of theirs.
        let stamp = self.vrchat_now().unwrap();
        assert!(self.publish_login(&stamp, |st, _| (st.allowed, st.user) = (true, Some(User { id: "usr_test".into(), ..Default::default() }))));
        sent
    }
    /// VRChat's session is this Orbiters member's grant and shows as signed in, as after a look.
    pub(super) fn test_signed_in(&self) {
        let (api, token) = self.vrchat_identity().unwrap();
        let id = (api.url(""), token);
        let now = self.vrchat_now();
        let mut rt = self.vrchat_login.lock().unwrap();
        rt.identity = Some(id.clone());
        rt.grants.entry(id).or_insert_with(new_grant).answered(&Ok(true), Instant::now());
        rt.status_for = now;
    }
    /// A read of the list that ends.
    pub(super) fn test_list(self: &Shared, list: orbiters_core::launcher::LauncherState) {
        let Some(session) = self.vrchat_session() else { return };
        self.launcher_commit_read(&session.by, list);
    }
    /// A sign-in (or a restore) that ends with `result`.
    pub(super) fn test_finish(self: &Shared, result: orbiters_core::vrchat_api::Result<Option<Login>>) {
        let Some((api, token, stamp)) = self.vrchat_ready() else { return };
        let generation = self.vrchat_login.lock().unwrap().generation;
        self.finish_vrchat(generation, &(api.url(""), token), &stamp, result, None);
    }
    pub(super) fn test_vrchat(&self, replies: Vec<(u16, serde_json::Value)>) -> (Arc<Mutex<Client>>, Arc<Mutex<Vec<(String, String)>>>) {
        self.test_account();
        let identity = self.vrchat_identity().unwrap();
        let mut rt = self.vrchat_login.lock().unwrap();
        assert!(rt.grant(Duration::from_secs(3600)));
        let id = (identity.0.url(""), identity.1);
        rt.identity = Some(id.clone());
        rt.grants.entry(id).or_insert_with(new_grant).answered(&Ok(true), Instant::now());
        rt.restored = true;
        let (client, calls) = orbiters_core::vrchat_api::test_client(rt.gate.clone(), replies);
        rt.client = Arc::new(Mutex::new(client));
        let client = rt.client.clone();
        drop(rt);
        let now = self.vrchat_now();
        self.vrchat_login.lock().unwrap().status_for = now;
        self.update(|s| { s.vrchat_login.allowed = true; s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() }); });
        (client, calls)
    }
}
