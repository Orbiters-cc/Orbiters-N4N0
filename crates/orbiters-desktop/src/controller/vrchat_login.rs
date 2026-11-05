//! Owner-gated, local VRChat session. Only the feature check goes to Orbiters.
use super::{Controller, Shared};
use crate::state::AccountState;
use orbiters_core::account::{Api, Server};
use orbiters_core::features::{FeatureCache, REFRESH};
use orbiters_core::vrchat_api::{Client, Error, Gate, Login, Secret, TwoFactor, User};
use serde::{Deserialize, Serialize};
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
    pub rgba: Vec<u8>,
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
pub(super) struct Runtime {
    pub(super) gate: Arc<Gate>,
    client: Arc<Mutex<Client>>,
    identity: Option<(String, String)>,
    feature: FeatureCache,
    attempted: Option<Instant>,
    generation: u64,
    busy: bool,
    restored: bool,
}
impl Default for Runtime {
    fn default() -> Self {
        let gate = Arc::new(Gate::default());
        Self {
            client: Arc::new(Mutex::new(Client::new(gate.clone()))),
            gate,
            identity: None,
            feature: FeatureCache::default(),
            attempted: None,
            generation: 0,
            busy: false,
            restored: false,
        }
    }
}
impl Runtime {
    /// Never revive an earlier grant: queued requests and data handles keep their revoked client.
    fn grant(&mut self) {
        self.gate.revoke();
        self.gate = Arc::new(Gate::for_duration(REFRESH));
        self.client = Arc::new(Mutex::new(Client::new(self.gate.clone())));
    }
    pub(super) fn suspend(&mut self) {
        self.gate.revoke();
        self.identity = None;
        self.feature = FeatureCache::default();
        self.attempted = None;
        self.generation += 1;
        self.busy = false;
        self.restored = false;
    }
}
impl Controller {
    fn vrchat_identity(&self) -> Option<(Api, String)> {
        let snap = self.view();
        if !matches!(snap.account.state, AccountState::SignedIn { .. }) || snap.account.development != (Server::current() == Server::Development) {
            return None;
        }
        self.account_api()
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
    fn vrchat_feature_tick(self: &Shared) {
        let account = self.vrchat_identity();
        let identity = account.as_ref().map(|(api, token)| (api.url(""), token.clone()));
        let (changed, check) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            let mut changed = rt.identity != identity;
            if changed {
                rt.gate.revoke();
                rt.identity = identity.clone();
                rt.feature = FeatureCache::default();
                rt.attempted = None;
                rt.generation += 1;
                rt.busy = false;
                rt.restored = false;
            }
            if rt.feature.due() && rt.restored {
                rt.gate.revoke();
                rt.generation += 1;
                rt.busy = false;
                rt.restored = false;
                changed = true;
            }
            let check = identity.is_some() && rt.feature.due() && rt.attempted.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
            if check {
                rt.attempted = Some(Instant::now());
            }
            (changed, check)
        };
        if changed {
            self.update(|s| s.vrchat_login = Status::default());
        }
        if !check {
            let (allowed, client) = {
                let rt = self.vrchat_login.lock().unwrap();
                (rt.feature.allowed(), rt.client.clone())
            };
            // A data lookup may have expired the session; reconcile without a network request or blocking a worker.
            if allowed {
                if let Ok(c) = client.try_lock() {
                    if !c.signed_in() {
                        self.update_if(|s| {
                            if s.vrchat_login.user.is_some() && !s.vrchat_login.busy {
                                s.vrchat_login.user = None;
                                s.vrchat_login.picture = None;
                                s.vrchat_login.error = Some(Error::Unauthorized.to_string());
                                true
                            } else {
                                false
                            }
                        });
                    }
                }
            }
            return;
        }
        let (Some((api, token)), Some(identity)) = (account, identity) else { return };
        let answer = api.vrchat_login_feature(&token);
        if !self.same_vrchat_identity(&identity) {
            return;
        }
        let fetched = answer.ok();
        let allowed = fetched.unwrap_or(false);
        let (client, restore, generation) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            if rt.identity.as_ref() != Some(&identity) {
                return;
            }
            if fetched.is_some() {
                rt.feature.set(allowed);
            } else {
                rt.feature = FeatureCache::default();
            }
            if allowed {
                rt.grant();
            } else {
                rt.gate.revoke();
            }
            if !allowed {
                rt.restored = false;
                rt.busy = false;
            }
            let restore = allowed && !rt.restored;
            if restore {
                rt.restored = true;
                rt.busy = true;
            }
            (rt.client.clone(), restore, rt.generation)
        };
        self.update(|s| {
            if allowed {
                s.vrchat_login.allowed = true;
                s.vrchat_login.busy = restore;
            } else {
                s.vrchat_login = Status::default();
            }
        });
        if restore {
            let (result, portrait) = {
                let mut c = client.lock().unwrap();
                let mut result = c.resume();
                let portrait = if let Ok(Some(Login::SignedIn(u))) = &result { c.image(u.picture()).ok().and_then(picture) } else { None };
                if matches!(result, Ok(Some(Login::SignedIn(_)))) && !c.signed_in() {
                    result = Err(Error::Unauthorized);
                }
                (result, portrait)
            };
            self.finish_vrchat(generation, &identity, result, portrait);
        }
    }
    /// Starts immediately; all HTTP, disk and image decoding work runs away from the window thread.
    pub fn vrchat_command(self: &Shared, command: Command) {
        let Some((api, token)) = self.vrchat_identity() else { return };
        let identity = (api.url(""), token);
        let (client, generation) = {
            let mut rt = self.vrchat_login.lock().unwrap();
            if rt.identity.as_ref() != Some(&identity) || !rt.feature.allowed() || !rt.gate.allowed() || rt.busy {
                return;
            }
            // An IPC peer cannot choose a verification method VRChat did not request.
            if let Command::Verify { method, .. } = &command {
                if !self.view().vrchat_login.methods.contains(method) {
                    return;
                }
            }
            rt.generation += 1;
            rt.busy = true;
            (rt.client.clone(), rt.generation)
        };
        self.update(|s| {
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
            let recovery = match &result {
                Err(Error::Storage) if signing_out => Recovery::RemoveSavedSession,
                Err(Error::Unavailable | Error::RateLimited(_) | Error::Storage) if !signing_out => Recovery::Resume,
                _ => Recovery::None,
            };
            let clear_session = signing_out || matches!(result, Err(Error::Unauthorized));
            let result = match result {
                Err(Error::Unauthorized) if signing_in => Err("The username or password was not accepted by VRChat.".to_string()),
                Err(Error::Storage) if signing_out => Err("The saved VRChat session could not be removed. Try signing out again.".into()),
                Err(e) if signing_out => Err(format!("Signed out on this computer. VRChat could not confirm remote sign-out: {e}")),
                r => r.map_err(|e| e.to_string()),
            };
            drop(c);
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
                return;
            }
            rt.busy = false;
            let status = &mut s.vrchat_login;
            status.busy = false;
            status.recovery = recovery;
            status.revision = status.revision.wrapping_add(1);
            if clear_session {
                status.user = None;
                status.picture = None;
                status.methods.clear();
            }
            match result {
                Ok(Some(Login::SignedIn(u))) => {
                    status.user = Some(u);
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
                    status.picture = None;
                    status.methods.clear();
                    status.error = None;
                }
                Err(e) => status.error = Some(e),
            }
        });
    }
    /// Call `avatar`, `friends`, `user`, `instance_alive` or `image` on a background thread.
    pub fn vrchat_source(&self) -> orbiters_core::vrchat_source::Source {
        let orbiters = self.vrchat_identity();
        let direct = orbiters.as_ref().and_then(|(api, token)| {
            let rt = self.vrchat_login.lock().unwrap();
            (rt.feature.allowed() && rt.identity.as_ref() == Some(&(api.url(""), token.clone()))).then(|| rt.client.clone())
        });
        let snap = self.view();
        let w = &snap.vrchat.world;
        let log = w.local_player.as_ref().and_then(|me| w.players.iter().find(|(_, n)| *n == me).map(|(id, _)| id.clone()));
        orbiters_core::vrchat_source::Source::new(direct, orbiters, log)
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
        runtime.grant();
        let old = runtime.gate.clone();
        let old_client = runtime.client.clone();
        assert!(old.allowed());
        runtime.suspend();
        runtime.grant();
        assert!(runtime.gate.allowed());
        assert!(!old.allowed());
        assert!(matches!(old_client.lock().unwrap().avatar("avtr_test"), Err(Error::Disabled)));
    }
    #[test]
    fn publishing_orbiters_sign_out_revokes_vrchat_immediately() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let gate = {
            let mut rt = c.vrchat_login.lock().unwrap();
            rt.grant();
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
