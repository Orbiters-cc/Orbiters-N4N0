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
        Self { client: Arc::new(Mutex::new(client)), gate, identity: None, grants: HashMap::new(), generation: 0, busy: false, restored: false, saved: None, ask }
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
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("vrchat-feature".into()).spawn(move || loop {
            let Some(ctl) = weak.upgrade() else { break };
            ctl.vrchat_feature_tick();
            drop(ctl);
            std::thread::sleep(Duration::from_millis(500));
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
            if rt.restored || rt.busy || !rt.allowed_for(id) {
                return None;
            }
            let held = rt.client.try_lock().ok().map(|c| c.holds_session())?;
            rt.restored = true;
            if held {
                return None;
            }
            rt.busy = true;
            rt.generation += 1;
            Some((rt.client.clone(), rt.generation))
        });
        if let (Some((client, generation)), Some(id)) = (restore, identity.as_ref()) {
            self.update(|s| s.vrchat_login.busy = true);
            let (result, portrait) = {
                let mut c = client.lock().unwrap();
                let mut result = c.resume();
                let portrait = if let Ok(Some(Login::SignedIn(u))) = &result { c.image(u.picture()).ok().and_then(picture) } else { None };
                if matches!(result, Ok(Some(Login::SignedIn(_)))) && !c.signed_in() {
                    result = Err(Error::Unauthorized);
                }
                (result, portrait)
            };
            if matches!(result, Ok(Some(_))) {
                self.vrchat_login.lock().unwrap().saved = Some(true);
            }
            self.finish_vrchat(generation, id, result, portrait);
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
        let (granted, waiting, busy, client) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            let granted = rt.identity.clone().is_some_and(|id| rt.allowed_for(&id));
            let refused = rt.identity.as_ref().and_then(|id| rt.grants.get(id)).is_some_and(|g| g.refused());
            // A client busy with a request holds a session (a sign-in, a read).
            let held = rt.client.try_lock().map(|c| c.holds_session()).unwrap_or(true);
            let waiting = !granted && !orbiters_gone && !refused && (held || rt.saved());
            (granted, waiting, rt.busy, rt.client.clone())
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
                // The runtime says whether a command or the restore is under way; one that was dropped (the grant changed
                // under it) leaves nothing busy.
                if !busy {
                    st.busy = false;
                    match user {
                        Some(u) => {
                            st.user = Some(u);
                            st.known = None;
                        }
                        None => {
                            if st.user.take().is_some() {
                                // A data lookup found the session ended (a 401 looked into).
                                st.picture = None;
                                st.error = Some(ended_text(ended.as_ref()));
                            }
                            if ended.is_some() {
                                st.known = ended;
                            } else if st.methods.is_empty() {
                                st.known = st.known.take().filter(|_| st.error.is_some());
                            }
                        }
                    }
                }
            } else if waiting {
                let known = st.user.take().or(st.known.take()).or(user).or(ended);
                *st = Status { revision: st.revision, waiting: true, known, picture: st.picture.take(), upload: st.upload.take(), ..Status::default() };
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
                } else if matches!(result, Ok(Some(Login::SignedIn(_)))) {
                    rt.saved = Some(true);
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
                status.known = status.user.take().or(status.known.take());
                status.picture = None;
                status.methods.clear();
            }
            match result {
                Ok(Some(Login::SignedIn(u))) => {
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
        *st = Status { revision: st.revision, waiting: true, known, picture: st.picture.take(), upload: st.upload.take(), ..Status::default() };
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
}

#[cfg(test)]
impl Controller {
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
