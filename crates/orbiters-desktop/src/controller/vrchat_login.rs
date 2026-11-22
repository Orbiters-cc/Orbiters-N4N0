//! Owner-gated, local VRChat session. Only the feature check goes to Orbiters.
use super::{Controller, Shared};
use crate::state::AccountState;
use orbiters_core::account::{Api, Server};
use orbiters_core::features::Grant;
use orbiters_core::vrchat_api::{Client, Error, Gate, Login, Secret, TwoFactor, User};
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
    /// A credential held for the environment chosen now: signed in, being checked, or Orbiters out of reach.
    Ready(Api, String),
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
        Self { client: Arc::new(Mutex::new(client)), gate, identity: None, grants: HashMap::new(), generation: 0, busy: false, restored: false, saved: None, ask, restore_again: None, restoring_now: false, bell: Arc::default(), signed_bell: Arc::default(), seen_account: None }
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
            AccountState::SignedIn { .. } | AccountState::Checking { .. } | AccountState::Offline { .. } | AccountState::SigningIn => match self.account_api() {
                Some((api, token)) => Orbiters::Ready(api, token),
                None if snap.account.state == AccountState::SigningIn => Orbiters::Gone,
                None => Orbiters::Changing,
            },
            AccountState::Unknown => Orbiters::Changing,
            AccountState::SignedOut | AccountState::Expired { .. } => Orbiters::Gone,
        }
    }
    pub(super) fn vrchat_identity(&self) -> Option<(Api, String)> {
        match self.vrchat_orbiters() {
            Orbiters::Ready(api, token) => Some((api, token)),
            _ => None,
        }
    }
    fn same_vrchat_identity(&self, identity: &(String, String)) -> bool {
        self.vrchat_identity().is_some_and(|(api, token)| api.url("") == identity.0 && token == identity.1)
    }
    pub(super) fn start_vrchat_login(self: &Shared) {
        self.vrchat_at_launch(read_account_cache());
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
    pub(super) fn vrchat_at_launch(&self, cached: Option<(User, Option<Picture>)>) {
        let saved = self.vrchat_login.lock().unwrap().saved();
        if !saved {
            return;
        }
        self.update(|s| {
            let st = &mut s.vrchat_login;
            if st.user.is_none() {
                st.restoring = true;
                if let Some((user, picture)) = cached {
                    st.known = Some(user);
                    st.picture = picture;
                }
            }
        });
    }
    /// One look: follows the Orbiters identity, asks Orbiters when due, hands the session to each new grant, reads the
    /// saved session once, and says what the session is.
    pub(super) fn vrchat_feature_tick(self: &Shared) {
        let orbiters = self.vrchat_orbiters();
        let (account, identity) = match &orbiters {
            Orbiters::Ready(api, token) => (Some((api.clone(), token.clone())), Some((api.url(""), token.clone()))),
            _ => (None, None),
        };
        let (ask, asker) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            let asker = rt.ask.clone();
            if rt.identity != identity {
                rt.gate.revoke();
                rt.identity = identity.clone();
                rt.generation += 1;
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
            let signed_in = rt.client.try_lock().ok().map(|c| c.signed_in_user().is_some())?;
            rt.restored = true;
            if signed_in {
                return None;
            }
            rt.busy = true;
            rt.restoring_now = true;
            rt.generation += 1;
            Some((rt.client.clone(), rt.generation))
        });
        if let (Some((client, generation)), Some(id)) = (restore, identity.as_ref()) {
            self.update(|s| {
                s.vrchat_login.busy = true;
                s.vrchat_login.restoring = true;
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
            self.finish_vrchat(generation, id, result, None);
            if let Some(user) = signed_in {
                self.vrchat_login.lock().unwrap().signed_bell.ring();
                let portrait = if user.picture().is_empty() { None } else { client.lock().unwrap().image(user.picture()).ok().and_then(picture) };
                let shown = self.update_if(|s| {
                    let st = &mut s.vrchat_login;
                    let same = st.user.as_ref().is_some_and(|u| u.id == user.id);
                    if same && portrait.is_some() {
                        st.picture = portrait.clone();
                    }
                    same && portrait.is_some()
                });
                write_account_cache(&user, if shown { portrait.as_ref() } else { None });
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
    fn settle_vrchat_status(&self, orbiters_gone: bool) {
        let (granted, waiting, busy, pending, client, signed_bell) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            let granted = rt.identity.clone().is_some_and(|id| rt.allowed_for(&id));
            let refused = rt.identity.as_ref().and_then(|id| rt.grants.get(id)).is_some_and(|g| g.refused());
            // A client busy with a request holds a session (a sign-in, a read).
            let held = rt.client.try_lock().map(|c| c.holds_session()).unwrap_or(true);
            let saved = held || rt.saved();
            let waiting = !granted && !orbiters_gone && !refused && saved;
            // The saved session is still to be read, is being read, or got no answer yet (r21).
            let pending = saved && (!rt.restored || rt.restoring_now || rt.restore_again.is_some());
            (granted, waiting, rt.busy, pending, rt.client.clone(), rt.signed_bell.clone())
        };
        let Ok(c) = client.try_lock() else { return };
        let user = c.signed_in_user();
        let ended = c.ended_user().cloned();
        drop(c);
        self.update_if(|s| {
            let st = &mut s.vrchat_login;
            let before = st.clone();
            if granted {
                st.allowed = true;
                st.waiting = false;
                st.restoring = st.user.is_none() && st.methods.is_empty() && (pending || (busy && st.restoring));
                // The runtime says whether a command or the restore is under way; one that was dropped (the grant changed
                // under it) leaves nothing busy.
                if !busy {
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
                }
            } else if waiting {
                let known = st.user.take().or(st.known.take()).or(user).or(ended);
                *st = Status { revision: st.revision, waiting: true, restoring: true, known, picture: st.picture.take(), upload: st.upload.take(), ..Status::default() };
            } else {
                let known = st.user.take().or(st.known.take());
                *st = Status { revision: st.revision, known, upload: st.upload.take(), ..Status::default() };
            }
            *st != before
        });
    }
    /// Starts immediately; all HTTP, disk and image decoding work runs away from the window thread.
    pub fn vrchat_command(self: &Shared, command: Command) {
        let Some((api, token)) = self.vrchat_identity() else { return };
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
            (rt.client.clone(), rt.generation)
        };
        let replacing_session = matches!(&command, Command::SignOut | Command::SignIn { .. } | Command::Cancel);
        self.update(|s| {
            if replacing_session { s.vrchat_login.user = None; s.vrchat_login.picture = None; s.vrchat_login.methods.clear(); }
            s.vrchat_login.revision = s.vrchat_login.revision.wrapping_add(1);
            s.vrchat_login.busy = true;
            s.vrchat_login.error = None;
        });
        let ctl = self.clone();
        std::thread::spawn(move || {
            let mut c = client.lock().unwrap();
            let signing_in = matches!(command, Command::SignIn { .. });
            let signing_out = matches!(command, Command::SignOut);
            let mut result = match command {
                Command::SignIn { username, password } => c.sign_in(username.trim(), &password.0).map(Some),
                Command::Verify { method, code } => c.verify(method, code.0.trim()).map(Some),
                Command::SignOut => c.sign_out().map(|_| None),
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
            let ended = c.ended_user().cloned();
            drop(c);
            {
                let mut rt = ctl.vrchat_login.lock().unwrap();
                if signing_out && result.is_ok() {
                    rt.saved = Some(false);
                    remove_account_cache();
                } else if let Ok(Some(Login::SignedIn(u))) = &result {
                    rt.saved = Some(true);
                    rt.signed_bell.ring();
                    write_account_cache(u, picture.as_ref());
                }
            }
            let recovery = match &result {
                Err(Error::Storage) if signing_out => Recovery::RemoveSavedSession,
                Err(Error::Unavailable | Error::RateLimited(_) | Error::Storage) if !signing_out => Recovery::Resume,
                _ => Recovery::None,
            };
            let clear_session = signing_out || matches!(result, Err(Error::Unauthorized));
            let result = match result {
                Err(Error::Unauthorized) if signing_in => Err(tr("The username or password was not accepted by VRChat.").to_string()),
                Err(Error::Unauthorized) => Err(ended_text(ended.as_ref())),
                Err(Error::Storage) if signing_out => Err(tr("The saved VRChat session could not be removed. Try signing out again.").into()),
                Err(e) if signing_out => Err(tr!("Signed out on this computer. VRChat could not confirm remote sign-out: {error}", error = e)),
                r => r.map_err(|e| e.to_string()),
            };
            ctl.finish_vrchat_text(generation, &identity, result, picture, clear_session, recovery);
        });
    }
    fn finish_vrchat(&self, generation: u64, identity: &(String, String), result: orbiters_core::vrchat_api::Result<Option<Login>>, picture: Option<Picture>) {
        let recovery = match &result {
            Err(Error::Unavailable | Error::RateLimited(_) | Error::Storage) => Recovery::Resume,
            _ => Recovery::None,
        };
        let clear_session = matches!(result, Err(Error::Unauthorized));
        self.finish_vrchat_text(generation, identity, result.map_err(|e| e.to_string()), picture, clear_session, recovery);
    }
    fn finish_vrchat_text(
        &self,
        generation: u64,
        identity: &(String, String),
        result: Result<Option<Login>, String>,
        picture: Option<Picture>,
        clear_session: bool,
        recovery: Recovery,
    ) {
        if !self.same_vrchat_identity(identity) {
            return;
        }
        self.update(|s| {
            let mut rt = self.vrchat_login.lock().unwrap();
            if rt.generation != generation || rt.identity.as_ref() != Some(identity) || !rt.gate.allowed() {
                // Not shown here: the next look says what the session is (it was handed to the new grant).
                return;
            }
            rt.busy = false;
            let status = &mut s.vrchat_login;
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
        });
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

/// At each publish, before the state goes out: nobody signed in to Orbiters (or its credential refused) stops VRChat's
/// grant at once and nothing of it shows as usable; the environment switching (its account not read yet) stops the
/// grant too, but the session shows as kept and waiting, not signed out (r20). The session itself is never dropped
/// here: it stays in memory and on disk for the next grant.
pub(super) fn follow_orbiters(s: &mut crate::state::Snapshot, rt: &mut Runtime) {
    // The Orbiters account changed (read, checked, signed in or out, the environment): VRChat's next look comes at once.
    let seen = (std::mem::discriminant(&s.account.state), s.account.development);
    if rt.seen_account != Some(seen) {
        rt.seen_account = Some(seen);
        rt.bell.ring();
    }
    let switching = s.account.development != (Server::current() == Server::Development) || s.account.state == AccountState::Unknown;
    let gone = matches!(s.account.state, AccountState::SignedOut | AccountState::Expired { .. });
    if !switching && !gone {
        return;
    }
    if rt.identity.is_some() || rt.gate.allowed() {
        rt.suspend();
    }
    let st = &mut s.vrchat_login;
    if gone {
        if st.allowed || st.user.is_some() || st.waiting || st.busy {
            let known = st.user.take().or(st.known.take());
            *st = Status { revision: st.revision, known, upload: st.upload.take(), ..Status::default() };
        }
    } else if st.allowed || st.user.is_some() {
        let known = st.user.take().or(st.known.take());
        *st = Status { revision: st.revision, waiting: true, restoring: true, known, picture: st.picture.take(), upload: st.upload.take(), ..Status::default() };
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
}

fn read_account_cache() -> Option<(User, Option<Picture>)> {
    let text = std::fs::read_to_string(account_cache_path()?).ok()?;
    let cached: CachedAccount = serde_json::from_str(&text).ok()?;
    let user: User = serde_json::from_value(serde_json::json!({"id": cached.id, "displayName": cached.name})).ok()?;
    Some((user, cached.picture))
}

fn write_account_cache(user: &User, picture: Option<&Picture>) {
    let Some(path) = account_cache_path() else { return };
    // A picture fetched before stays when this time none came.
    let picture = picture.cloned().or_else(|| read_account_cache().filter(|(u, _)| u.id == user.id).and_then(|(_, p)| p));
    let cached = CachedAccount { id: user.id.clone(), name: user.display_name.clone(), picture };
    if let (Some(dir), Ok(text)) = (path.parent(), serde_json::to_vec(&cached)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = orbiters_core::write_atomic(&path, &text);
    }
}

fn remove_account_cache() {
    if let Some(path) = account_cache_path() {
        let _ = std::fs::remove_file(path);
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
            rt.account = Some(orbiters_core::account::Account { token: token.into(), user: None, username: Some("tester".into()), avatar_url: None, features: None });
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
        assert!(status.waiting && status.user.is_none() && status.known.as_ref().map(|u| u.id.as_str()) == Some("usr_test"), "{status:?}");
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
        assert_eq!(status.known.map(|u| u.id), tester());
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
}

#[cfg(test)]
impl Controller {
    /// A saved session at launch (r21 timing tests): `client` holds its cookie and nobody is signed in yet; Orbiters is
    /// asked through `ask`.
    pub(super) fn test_saved_vrchat(&self, client: Client, ask: impl Fn(&Api, &str) -> anyhow::Result<bool> + Send + Sync + 'static) {
        let mut rt = self.vrchat_login.lock().unwrap();
        rt.client = Arc::new(Mutex::new(client));
        rt.restored = false;
        rt.ask = Arc::new(ask);
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
        self.update(|s| { s.vrchat_login.allowed = true; s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() }); });
        (client, calls)
    }
}
