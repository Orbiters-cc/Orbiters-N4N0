//! Who decides whether the bridge exists. A few times a second: who is signed in on the server this build talks to, whether
//! that server said yes for that account recently enough, and whether Settings has it on. All three, or nothing listens.
//! The answer lives in this process's memory only, and nothing in a settings file can stand in for it: a person editing a
//! file can turn it off, and can never turn it on for an account the server does not say has it.

use orbiters_core::locks::*;
use super::clients::{Change, Client, Registrations, Wanted};
use super::mcp::Server;
use super::person::Listener;
use super::{Bridge, Command, Status};
use orbiters_core::features::Grant;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often the server is asked again while it says yes.
pub const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long the last yes counts when the server cannot be reached: "not reachable with no recent proof" means no bridge.
pub const PROOF_LASTS: Duration = Duration::from_secs(30 * 60);
/// The port the bridge first tries, so that a client's configuration stays good; another when something else has it.
const FIRST_PORT: u16 = 47615;
/// Tells this file's use of the sealing from others'.
const ENTROPY: &[u8] = b"Orbiters.N4N0 VRChat bridge";

/// What the keeper asks the world.
pub trait Flag: Send + Sync {
    /// Who is signed in, and on which server: anything that changes when either does. `None`: nobody, so no proof.
    fn identity(&self) -> Option<String>;
    /// Asks the server, for `identity` ([`Flag::identity`]) and no other: an error when it is not who is signed in any more. May
    /// take long: it runs on a thread of its own.
    fn ask(&self, identity: &str) -> anyhow::Result<bool>;
    /// Settings has it on.
    fn enabled(&self) -> bool;
    /// Stop: Settings has it off from now on.
    fn turn_off(&self);
}

/// The port and token a client is given. They last across restarts, in a file of this user's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub port: u16,
    pub token: String,
}

impl Keys {
    fn new_token() -> String {
        // UUID v4s come from the system's random source: 244 random bits.
        format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
    }

    fn file() -> PathBuf {
        orbiters_core::paths::config_dir().join("vrchat-bridge.dat")
    }

    /// The saved ones: `Ok(None)` when there are none yet (no file), `Err` when a file is there and could not be read or
    /// opened (Windows' protection unavailable, a file being written, another user's): that is not "none", and new keys
    /// must not replace them, or every agent given the old token would be refused.
    pub fn load(file: &std::path::Path) -> Result<Option<Self>, String> {
        match orbiters_core::sealed::read::<Keys>(file, ENTROPY) {
            Ok(Some(keys)) if keys.token.is_empty() => Err("no token in it".into()), // i18n: not shown (log detail)
            Ok(keys) => Ok(keys),
            Err(e) => Err(format!("{e:#}")),
        }
    }

    /// The saved ones, or, when there are none yet, new ones once they are saved: keys that could not be kept are never
    /// handed out (the next start would make others).
    pub fn load_or_create(file: &std::path::Path) -> Result<Self, String> {
        match Self::load(file)? {
            Some(keys) => Ok(keys),
            None => {
                let keys = Keys { port: FIRST_PORT, token: Self::new_token() };
                keys.save(file)?;
                Ok(keys)
            }
        }
    }

    /// New keys in place of keys that cannot be read (an explicit reset from Settings): the unreadable file is kept aside,
    /// never deleted (`<name>.unreadable-<Unix s>`), then new keys are saved. Readable keys get a new token and keep their
    /// port. The keys, and where the old file went.
    pub fn reset(file: &std::path::Path) -> Result<(Self, Option<PathBuf>), String> {
        let (keys, aside) = match Self::load(file) {
            Ok(Some(keys)) => (Keys { token: Self::new_token(), ..keys }, None),
            Ok(None) => (Keys { port: FIRST_PORT, token: Self::new_token() }, None),
            Err(_) => {
                let name = file.file_name().map_or_else(|| "keys".into(), |n| n.to_string_lossy().into_owned());
                let aside = file.with_file_name(format!("{name}.unreadable-{}", time::OffsetDateTime::now_utc().unix_timestamp()));
                std::fs::rename(file, &aside).map_err(|e| e.to_string())?;
                (Keys { port: FIRST_PORT, token: Self::new_token() }, Some(aside))
            }
        };
        keys.save(file)?;
        Ok((keys, aside))
    }

    pub fn save(&self, file: &std::path::Path) -> Result<(), String> {
        orbiters_core::sealed::write(file, self, ENTROPY).map_err(|e| format!("{e:#}"))
    }
}

struct Open {
    server: Server,
    /// Held for as long as it listens; dropped, the person is no longer heard, and nothing is left to hear them for.
    _listener: Listener,
    hotkey_taken: bool,
    token: String,
}

struct Inner {
    identity: Option<String>,
    /// Bumped with every identity change: an answer for an older one is dropped.
    generation: u64,
    grant: Grant,
    open: Option<Open>,
    problem: Option<String>,
    /// The last try to open found the keys' file unreadable.
    keys_unreadable: bool,
}

/// Follows the three conditions.
pub struct Keeper {
    pub bridge: Arc<Bridge>,
    flag: Arc<dyn Flag>,
    keys_file: PathBuf,
    inner: Mutex<Inner>,
    /// Asking takes a thread: one at a time.
    asking: AtomicBool,
    published: Box<dyn Fn(Status) + Send + Sync>,
    /// How to hear the person: Raw Input for real, nothing in tests (`None` from it fails closed).
    hear: Hear,
    /// The agents the bridge can be added to: the real ones in the service, none in the lab and in tests.
    pub(crate) registrations: OnceLock<Arc<Registrations>>,
    /// The bridge just opened, or its token changed: what the agents have is looked at, and fixed.
    repair_due: AtomicBool,
    /// Held around every first creation or change of the keys' file: the listener's start and an agent's registration never
    /// make two different tokens at once.
    keys_lock: Mutex<()>,
}

/// How to hear the person: `None` when that cannot be done.
type Hear = Box<dyn Fn(&Arc<Bridge>) -> Option<Listener> + Send + Sync>;

impl Keeper {
    pub fn new(
        bridge: Arc<Bridge>,
        flag: Arc<dyn Flag>,
        keys_file: PathBuf,
        refresh: Duration,
        lifetime: Duration,
        published: Box<dyn Fn(Status) + Send + Sync>,
        hear: Hear,
    ) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            flag,
            keys_file,
            inner: Mutex::new(Inner { identity: None, generation: 0, grant: Grant::new(refresh, lifetime), open: None, problem: None, keys_unreadable: false }),
            asking: AtomicBool::new(false),
            published,
            hear,
            registrations: OnceLock::new(),
            repair_due: AtomicBool::new(false),
            keys_lock: Mutex::new(()),
        })
    }

    /// The real one: the file in the settings folder, and Raw Input.
    pub fn for_service(bridge: Arc<Bridge>, flag: Arc<dyn Flag>, published: Box<dyn Fn(Status) + Send + Sync>) -> Arc<Self> {
        let keeper = Self::new(bridge.clone(), flag, Keys::file(), ASK_EVERY, PROOF_LASTS, published, Box::new(super::hear_person));
        let _ = keeper.registrations.set(Registrations::real(Box::new(move |ok, line| bridge.log(ok, line))));
        keeper
    }

    /// What an agent's entry should say now: the open listener's own port and token, else the saved keys; none while they
    /// cannot be read (an entry is never given a token nothing listens with).
    fn wanted(&self) -> Option<Wanted> {
        let open = self.inner.locked().open.as_ref().map(|o| (o.server.port(), o.token.clone()));
        let (port, token, listening) = match open {
            Some((port, token)) => (port, token, true),
            None => {
                let keys = {
                    let _one = self.keys_lock.locked();
                    Keys::load_or_create(&self.keys_file).ok()?
                };
                (keys.port, keys.token, false)
            }
        };
        Some(Wanted { address: address_of(port), token, listening })
    }

    fn clients(&self, act: impl FnOnce(&Arc<Registrations>, Wanted)) {
        if let (Some(registrations), Some(wanted)) = (self.registrations.get(), self.wanted()) {
            act(registrations, wanted);
        }
    }

    /// Runs on a thread of its own for the life of the process.
    pub fn start(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("bridge-keeper".into()).spawn(move || loop {
            let Some(keeper) = weak.upgrade() else { break };
            keeper.tick();
            drop(keeper);
            std::thread::sleep(Duration::from_millis(250));
        });
    }

    /// One look at the three conditions, and acting on what they say, then what Settings shows, a few times a second.
    pub fn tick(self: &Arc<Self>) {
        let now = Instant::now();
        let ask = {
            let mut inner = self.inner.locked();
            self.follow_identity(&mut inner);
            let ask = inner.identity.clone().filter(|_| inner.grant.due(now) && !self.asking.swap(true, Ordering::Relaxed));
            ask.map(|identity| (identity, inner.generation))
        };
        if let Some((identity, generation)) = ask {
            let keeper = self.clone();
            let spawned = std::thread::Builder::new().name("bridge-flag".into()).spawn(move || {
                let answer = keeper.flag.ask(&identity);
                keeper.answered(generation, &answer);
                keeper.asking.store(false, Ordering::Relaxed);
                keeper.tick();
            });
            if spawned.is_err() {
                self.asking.store(false, Ordering::Relaxed);
            }
        }
        let mut inner = self.inner.locked();
        self.follow_identity(&mut inner);
        let allowed = inner.identity.is_some() && inner.grant.allowed(now);
        let wanted = allowed && self.flag.enabled();
        if wanted && inner.open.is_none() {
            self.open(&mut inner);
        } else if !wanted && inner.open.is_some() {
            // Turned off in Settings is the person's; Orbiters' grant lapsing is not.
            let why = if allowed { tr("The bridge was turned off.") } else { tr("Orbiters no longer says this account has the bridge: it closed.") };
            self.close(&mut inner, why, allowed);
        } else if !wanted {
            inner.problem = None;
        }
        drop(inner);
        if self.repair_due.swap(false, Ordering::Relaxed) {
            self.clients(|clients, wanted| clients.repair(wanted));
        }
        self.publish();
    }

    /// Who is signed in now, read with what it decides held: another than before, and nothing of the last one's is left.
    fn follow_identity(&self, inner: &mut Inner) {
        let identity = self.flag.identity();
        if inner.identity != identity {
            inner.identity = identity;
            inner.grant.forget();
            inner.generation += 1;
            self.close(inner, tr("The Orbiters account or server changed: the bridge closed."), false);
        }
    }

    /// What the server answered for the identity of `generation`: taken for that one alone.
    fn answered(&self, generation: u64, answer: &anyhow::Result<bool>) {
        let mut inner = self.inner.locked();
        self.follow_identity(&mut inner);
        if inner.generation == generation {
            inner.grant.answered(answer, Instant::now());
        }
    }

    fn open(&self, inner: &mut Inner) {
        let loaded = {
            let _one = self.keys_lock.locked();
            Keys::load_or_create(&self.keys_file)
        };
        let mut keys = match loaded {
            Ok(keys) => {
                inner.keys_unreadable = false;
                keys
            }
            Err(e) => {
                // Tried again at the next look (the computer unlocked, the file written whole); Settings offers a reset.
                inner.keys_unreadable = true;
                inner.problem = Some(tr!("The bridge's keys could not be read ({error}), so it stays closed until they can be, or until you reset the token.", error = e));
                return;
            }
        };
        let Some(listener) = (self.hear)(&self.bridge) else {
            inner.problem = Some(tr("Windows would not let the bridge hear the keyboard and mouse, so it stays closed: an agent that cannot tell the person is here may not run.").into());
            return;
        };
        if let Err(e) = self.bridge.start_guard() {
            inner.problem = Some(tr!("{error}, so the bridge stays closed: input could stay down if the app crashed.", error = e));
            return;
        }
        self.bridge.tell_guard_target();
        let server = Server::start(self.bridge.clone(), keys.port, keys.token.clone()).or_else(|_| {
            // Something else has that port: another, kept for next time, and told in Settings.
            let server = Server::start(self.bridge.clone(), 0, keys.token.clone())?;
            keys.port = server.port();
            if let Err(e) = keys.save(&self.keys_file) {
                eprintln!("the bridge's port was not saved: {e}");
            }
            Ok::<Server, std::io::Error>(server)
        });
        match server {
            Ok(server) => {
                let hotkey_taken = listener.hotkey_taken;
                inner.open = Some(Open { server, _listener: listener, hotkey_taken, token: keys.token });
                inner.problem = None;
                self.bridge.log(true, tr!("Listening on 127.0.0.1:{port}.", port = keys.port));
                // Whatever moved since an agent was told, it is told now.
                self.repair_due.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                self.bridge.stop_guard();
                inner.problem = Some(tr!("The bridge could not listen: {error}", error = e));
            }
        }
    }

    /// Everything of it goes: input released, what runs ended, the port closed, the person no longer listened to. Only the
    /// person's own Stop (`by_person`) makes the main VRChat theirs (BRIDGE-5).
    fn close(&self, inner: &mut Inner, why: &str, by_person: bool) {
        if inner.open.is_none() && by_person {
            // Closed already (Orbiters' grant lapsed, say), the bridge kept its VRChat: the person's Stop makes it theirs all
            // the same (verify-n4n0 4).
            self.bridge.take_over(why);
            return;
        }
        if let Some(open) = inner.open.take() {
            if by_person {
                self.bridge.take_over(why);
            } else {
                self.bridge.wind_down(why);
            }
            drop(open);
            self.bridge.stop_guard();
        }
    }

    /// What Settings asked.
    pub fn command(self: &Arc<Self>, command: Command) {
        match command {
            Command::Stop => {
                self.flag.turn_off();
                let mut inner = self.inner.locked();
                self.close(&mut inner, tr("Stopped from Settings: control is back with the person."), true);
                drop(inner);
                // Whatever paused it is over with it: the next time it is on, it starts clean.
                self.bridge.presence.locked().resume();
            }
            Command::Pause => self.bridge.hold(true),
            Command::Resume => self.bridge.hold(false),
            Command::NewToken => {
                // Only a token that was kept replaces the old one.
                let renewed = Keys::load(&self.keys_file).map(|keys| keys.unwrap_or(Keys { port: FIRST_PORT, token: String::new() })).and_then(|mut keys| {
                    keys.token = Keys::new_token();
                    keys.save(&self.keys_file)
                });
                if let Err(e) = renewed {
                    self.bridge.log(false, tr!("The bridge's keys could not be read ({error}), so it stays closed until they can be.", error = e));
                    return;
                }
                let mut inner = self.inner.locked();
                if inner.open.is_some() {
                    self.close(&mut inner, tr("A new token: the agents it was added to are updated, others have to be given it again."), false);
                }
                // Also while closed: nothing else would ever tell them.
                self.repair_due.store(true, Ordering::Relaxed);
            }
            Command::ResetToken => {
                // Only while closed: an open listener's keys were read, and New token is the way there.
                if self.inner.locked().open.is_some() {
                    return self.tick();
                }
                let reset = {
                    let _one = self.keys_lock.locked();
                    Keys::reset(&self.keys_file)
                };
                match reset {
                    Ok((_, aside)) => {
                        let kept = aside.map(|a| a.display().to_string()).unwrap_or_default();
                        self.bridge.log(true, tr!("A new token: the bridge's unreadable keys were kept aside ({path}); the agents it was added to are updated.", path = kept));
                        self.inner.locked().keys_unreadable = false;
                        self.repair_due.store(true, Ordering::Relaxed);
                    }
                    Err(e) => self.bridge.log(false, tr!("The bridge's keys could not be reset: {error}", error = e)),
                }
            }
            Command::Client { client, change } => self.change_client(client, change),
            Command::CheckClients => self.clients(|clients, wanted| clients.check(wanted)),
            // Asked by this process, the one whose screenshots need it (macOS judges the process that captures).
            Command::AllowScreenRecording => super::mac::ask_screen_recording(),
        }
        self.tick();
    }

    fn change_client(&self, client: Client, change: Change) {
        self.clients(|clients, wanted| clients.change(client, change, wanted));
    }

    /// Ends it all when the app quits.
    pub fn shutdown(&self) {
        let mut inner = self.inner.locked();
        self.close(&mut inner, tr("The app is closing."), true);
    }

    /// The address clients use, when open.
    pub fn port(&self) -> Option<u16> {
        self.inner.locked().open.as_ref().map(|o| o.server.port())
    }

    fn publish(&self) {
        let mut status = Status::default();
        {
            let inner = self.inner.locked();
            status.allowed = inner.identity.is_some() && inner.grant.allowed(Instant::now());
            status.problem = inner.problem.clone();
            status.keys_unreadable = inner.keys_unreadable && inner.open.is_none();
            if let Some(open) = inner.open.as_ref() {
                status.listening = true;
                status.address = Some(address_of(open.server.port()));
                status.token = Some(open.token.clone());
                status.hotkey_taken = open.hotkey_taken;
            }
        }
        if status.allowed {
            self.bridge.status_into(&mut status);
            status.screen_recording = super::mac::screen_recording();
            if let Some(registrations) = self.registrations.get() {
                status.clients = registrations.statuses();
            }
        }
        (self.published)(status);
    }
}

/// The address a client is given.
fn address_of(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_last_and_are_not_written_in_the_clear() {
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-keys-{}", uuid::Uuid::new_v4().simple()));
        let file = dir.join("vrchat-bridge.dat");
        let keys = Keys::load_or_create(&file).unwrap();
        assert_eq!(keys.port, FIRST_PORT);
        assert_eq!(keys.token.len(), 64);
        assert_eq!(Keys::load_or_create(&file).unwrap(), keys, "the same across restarts");
        let on_disk = std::fs::read(&file).unwrap();
        assert!(!String::from_utf8_lossy(&on_disk).contains(&keys.token));
        // The verifier's case: a file that cannot be read (Windows' protection unavailable, a file half written) is not
        // "no keys": nothing replaces it, nothing new is handed out, and once readable the old keys are there.
        let before = std::fs::read(&file).unwrap();
        std::fs::write(&file, b"nonsense").unwrap();
        assert!(Keys::load_or_create(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"nonsense", "left as it was");
        std::fs::write(&file, &before).unwrap();
        assert_eq!(Keys::load_or_create(&file).unwrap(), keys);
        // Keys that cannot be saved are never handed out.
        let nowhere = dir.join("a-file-not-a-folder");
        std::fs::write(&nowhere, b"x").unwrap();
        assert!(Keys::load_or_create(&nowhere.join("vrchat-bridge.dat")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// BRG-2: an answer is the identity's it was asked for, whenever the account changes: while it is asked, while it waits to be
    /// taken, and as a look at the conditions is under way. The other account's grant is nobody's yes but its own.
    #[test]
    fn an_answer_is_its_own_accounts_alone() {
        use std::sync::mpsc;
        struct Switching {
            identity: Mutex<Option<String>>,
            asked: Mutex<mpsc::Sender<String>>,
            answers: Mutex<mpsc::Receiver<bool>>,
        }
        impl Flag for Switching {
            fn identity(&self) -> Option<String> {
                self.identity.lock().unwrap().clone()
            }
            fn ask(&self, identity: &str) -> anyhow::Result<bool> {
                self.asked.lock().unwrap().send(identity.to_string())?;
                Ok(self.answers.lock().unwrap().recv()?)
            }
            fn enabled(&self) -> bool {
                false
            }
            fn turn_off(&self) {}
        }
        let (asked, asks) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        let flag = Arc::new(Switching { identity: Mutex::new(Some("a".into())), asked: Mutex::new(asked), answers: Mutex::new(answers) });
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-flag-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let keeper = Keeper::new(Bridge::new(super::super::lab::LabGame::new(&dir, None), None), flag.clone(), dir.join("keys.dat"), ASK_EVERY, PROOF_LASTS, Box::new(|_| {}), Box::new(|_| None));
        let allowed = || keeper.inner.lock().unwrap().grant.allowed(Instant::now());
        let wait = Duration::from_secs(20);
        keeper.tick();
        assert_eq!(asks.recv_timeout(wait).unwrap(), "a");
        // The answer comes, and the account changes before it is taken.
        {
            let mut held = keeper.inner.lock().unwrap();
            answer.send(true).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            *flag.identity.lock().unwrap() = Some("b".into());
            // (As a look at the conditions under way does.)
            keeper.follow_identity(&mut held);
            drop(held);
        }
        // Asked for itself, whoever was asked for before; and nothing is allowed meanwhile.
        assert_eq!(asks.recv_timeout(wait).unwrap(), "b");
        assert!(!allowed(), "the other account's yes is not this one's");
        answer.send(false).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!allowed());
        // Its own yes is.
        *flag.identity.lock().unwrap() = Some("a".into());
        keeper.tick();
        assert_eq!(asks.recv_timeout(wait).unwrap(), "a");
        answer.send(true).unwrap();
        let until = Instant::now() + wait;
        while !allowed() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(allowed());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// M11 (Windows' recovery, shared code): a keys' file that can never be read is reset on request: kept aside, never
    /// deleted, new keys saved and loaded from then on; readable keys keep their port and get a new token.
    #[test]
    fn unreadable_keys_are_reset_on_request_and_kept_aside() {
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-reset-{}", uuid::Uuid::new_v4().simple()));
        let file = dir.join("vrchat-bridge.dat");
        let first = Keys::load_or_create(&file).unwrap();
        let (renewed, aside) = Keys::reset(&file).unwrap();
        assert_eq!((renewed.port, aside), (first.port, None));
        assert_ne!(renewed.token, first.token);
        assert_eq!(Keys::load(&file).unwrap(), Some(renewed.clone()));
        std::fs::write(&file, b"sealed by another user").unwrap();
        assert!(Keys::load_or_create(&file).is_err(), "unreadable: never replaced by itself");
        let (fresh, aside) = Keys::reset(&file).unwrap();
        let aside = aside.expect("kept aside");
        assert_eq!(std::fs::read(&aside).unwrap(), b"sealed by another user");
        assert_eq!(Keys::load_or_create(&file).unwrap(), fresh, "the new keys from now on");
        let _ = std::fs::remove_dir_all(dir);
    }
}
