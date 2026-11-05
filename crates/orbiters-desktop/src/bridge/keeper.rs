//! Who decides whether the bridge exists. A few times a second: who is signed in on the server this build talks to, whether
//! that server said yes for that account recently enough, and whether Settings has it on. All three, or nothing listens.
//! The answer lives in this process's memory only, and nothing in a settings file can stand in for it: a person editing a
//! file can turn it off, and can never turn it on for an account the server does not say has it.

use super::mcp::Server;
use super::person::Listener;
use super::{Bridge, Command, Status};
use orbiters_core::features::Grant;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the server is asked again while it says yes.
pub const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long the last yes counts when the server cannot be reached: "not reachable with no recent proof" means no bridge.
pub const PROOF_LASTS: Duration = Duration::from_secs(30 * 60);
/// The port the bridge first tries, so that a client's configuration stays good; another when something else has it.
const FIRST_PORT: u16 = 47615;
/// Tells this file's use of Windows' protection from others'.
const ENTROPY: &[u8] = b"Orbiters.N4N0 VRChat bridge";

/// What the keeper asks the world.
pub trait Flag: Send + Sync {
    /// Who is signed in, and on which server: anything that changes when either does. `None`: nobody, so no proof.
    fn identity(&self) -> Option<String>;
    /// Asks the server, for the identity at the time. May take long: it runs on a thread of its own.
    fn ask(&self) -> anyhow::Result<bool>;
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

    /// The saved ones, or new ones, saved.
    pub fn load_or_create(file: &std::path::Path) -> Self {
        let saved = std::fs::read(file)
            .ok()
            .and_then(|sealed| orbiters_core::open_for_user(&sealed, ENTROPY).ok())
            .and_then(|plain| serde_json::from_slice::<Keys>(&plain).ok());
        match saved {
            Some(keys) if !keys.token.is_empty() => keys,
            _ => {
                let keys = Keys { port: FIRST_PORT, token: Self::new_token() };
                keys.save(file);
                keys
            }
        }
    }

    pub fn save(&self, file: &std::path::Path) {
        let written =
            serde_json::to_vec(self).map_err(anyhow::Error::from).and_then(|plain| orbiters_core::seal_for_user(&plain, ENTROPY)).and_then(|sealed| {
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                orbiters_core::write_atomic_private(file, &sealed)?;
                Ok(())
            });
        if let Err(e) = written {
            eprintln!("the bridge's keys were not saved: {e:#}");
        }
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
    grant: Grant,
    open: Option<Open>,
    problem: Option<String>,
}

/// Follows the three conditions.
pub struct Keeper {
    pub bridge: Arc<Bridge>,
    flag: Arc<dyn Flag>,
    keys_file: PathBuf,
    inner: Mutex<Inner>,
    /// Asking takes a thread: one at a time.
    asking: AtomicBool,
    /// Bumped with every identity change: an answer for an older one is dropped.
    generation: AtomicU64,
    published: Box<dyn Fn(Status) + Send + Sync>,
    /// How to hear the person: Raw Input for real, nothing in tests (`None` from it fails closed).
    hear: Hear,
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
            inner: Mutex::new(Inner { identity: None, grant: Grant::new(refresh, lifetime), open: None, problem: None }),
            asking: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            published,
            hear,
        })
    }

    /// The real one: the file in the settings folder, and Raw Input.
    pub fn for_service(bridge: Arc<Bridge>, flag: Arc<dyn Flag>, published: Box<dyn Fn(Status) + Send + Sync>) -> Arc<Self> {
        Self::new(bridge, flag, Keys::file(), ASK_EVERY, PROOF_LASTS, published, Box::new(super::hear_person))
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
        let identity = self.flag.identity();
        let ask = {
            let mut inner = self.inner.lock().unwrap();
            if inner.identity != identity {
                inner.identity = identity.clone();
                inner.grant.forget();
                self.generation.fetch_add(1, Ordering::Relaxed);
                self.close(&mut inner, "The Orbiters account or server changed: the bridge closed.");
            }
            identity.is_some() && inner.grant.due(now) && !self.asking.swap(true, Ordering::Relaxed)
        };
        if ask {
            let keeper = self.clone();
            let generation = self.generation.load(Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name("bridge-flag".into()).spawn(move || {
                let answer = keeper.flag.ask();
                if keeper.generation.load(Ordering::Relaxed) == generation {
                    keeper.inner.lock().unwrap().grant.answered(&answer, Instant::now());
                }
                keeper.asking.store(false, Ordering::Relaxed);
                keeper.tick();
            });
            if spawned.is_err() {
                self.asking.store(false, Ordering::Relaxed);
            }
        }
        let mut inner = self.inner.lock().unwrap();
        let allowed = identity.is_some() && inner.grant.allowed(now);
        let wanted = allowed && self.flag.enabled();
        if wanted && inner.open.is_none() {
            self.open(&mut inner);
        } else if !wanted && inner.open.is_some() {
            let why = if allowed { "The bridge was turned off." } else { "Orbiters no longer says this account has the bridge: it closed." };
            self.close(&mut inner, why);
        } else if !wanted {
            inner.problem = None;
        }
        drop(inner);
        self.publish();
    }

    fn open(&self, inner: &mut Inner) {
        let mut keys = Keys::load_or_create(&self.keys_file);
        let Some(listener) = (self.hear)(&self.bridge) else {
            inner.problem = Some(
                "Windows would not let the bridge hear the keyboard and mouse, so it stays closed: an agent that cannot tell the person is here may not run."
                    .into(),
            );
            return;
        };
        if let Err(e) = self.bridge.start_guard() {
            inner.problem = Some(format!("{e}, so the bridge stays closed: input could stay down if the app crashed."));
            return;
        }
        self.bridge.tell_guard_target();
        let server = Server::start(self.bridge.clone(), keys.port, keys.token.clone()).or_else(|_| {
            // Something else has that port: another, kept for next time, and told in Settings.
            let server = Server::start(self.bridge.clone(), 0, keys.token.clone())?;
            keys.port = server.port();
            keys.save(&self.keys_file);
            Ok::<Server, std::io::Error>(server)
        });
        match server {
            Ok(server) => {
                let hotkey_taken = listener.hotkey_taken;
                inner.open = Some(Open { server, _listener: listener, hotkey_taken, token: keys.token });
                inner.problem = None;
                self.bridge.log(true, format!("Listening on 127.0.0.1:{}.", keys.port));
            }
            Err(e) => {
                self.bridge.stop_guard();
                inner.problem = Some(format!("The bridge could not listen: {e}"));
            }
        }
    }

    /// Everything of it goes: input released, what runs ended, the port closed, the person no longer listened to.
    fn close(&self, inner: &mut Inner, why: &str) {
        if let Some(open) = inner.open.take() {
            self.bridge.take_over(why);
            drop(open);
            self.bridge.stop_guard();
        }
    }

    /// What Settings asked.
    pub fn command(self: &Arc<Self>, command: Command) {
        match command {
            Command::Stop => {
                self.flag.turn_off();
                let mut inner = self.inner.lock().unwrap();
                self.close(&mut inner, "Stopped from Settings: control is back with the person.");
                drop(inner);
                // Whatever paused it is over with it: the next time it is on, it starts clean.
                self.bridge.presence.lock().unwrap().resume();
            }
            Command::Pause => self.bridge.hold(true),
            Command::Resume => self.bridge.hold(false),
            Command::NewToken => {
                let mut keys = Keys::load_or_create(&self.keys_file);
                keys.token = Keys::new_token();
                keys.save(&self.keys_file);
                let mut inner = self.inner.lock().unwrap();
                if inner.open.is_some() {
                    self.close(&mut inner, "A new token: clients have to be given it again.");
                }
            }
        }
        self.tick();
    }

    /// Ends it all when the app quits.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        self.close(&mut inner, "The app is closing.");
    }

    /// The address clients use, when open.
    pub fn port(&self) -> Option<u16> {
        self.inner.lock().unwrap().open.as_ref().map(|o| o.server.port())
    }

    fn publish(&self) {
        let mut status = Status::default();
        {
            let inner = self.inner.lock().unwrap();
            status.allowed = inner.identity.is_some() && inner.grant.allowed(Instant::now());
            status.problem = inner.problem.clone();
            if let Some(open) = inner.open.as_ref() {
                status.listening = true;
                status.address = Some(format!("http://127.0.0.1:{}/mcp", open.server.port()));
                status.token = Some(open.token.clone());
                status.hotkey_taken = open.hotkey_taken;
            }
        }
        if status.allowed {
            self.bridge.status_into(&mut status);
        }
        (self.published)(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_last_and_are_not_written_in_the_clear() {
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-keys-{}", uuid::Uuid::new_v4().simple()));
        let file = dir.join("vrchat-bridge.dat");
        let keys = Keys::load_or_create(&file);
        assert_eq!(keys.port, FIRST_PORT);
        assert_eq!(keys.token.len(), 64);
        assert_eq!(Keys::load_or_create(&file), keys, "the same across restarts");
        if cfg!(windows) {
            let on_disk = std::fs::read(&file).unwrap();
            assert!(!String::from_utf8_lossy(&on_disk).contains(&keys.token));
        }
        // A file that cannot be read gets new ones rather than none.
        std::fs::write(&file, b"nonsense").unwrap();
        assert_ne!(Keys::load_or_create(&file).token, keys.token);
        let _ = std::fs::remove_dir_all(dir);
    }
}
