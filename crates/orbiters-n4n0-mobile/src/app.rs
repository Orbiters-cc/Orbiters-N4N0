//! The app's core, one per process: started by whatever keeps the app running (Android's foreground service, the window
//! on iOS). It runs OSC with VRChat on the Quest (`orbiters_core::osc`, LAN mode: the services listen on
//! the phone's Wi-Fi address and are announced there, VRChat finds them and sends what its avatar does), N4N0's runtime
//! (`orbiters_n4n0`: saved states, carried parameters) and the Orbiters sign-in, and keeps a [`Snapshot`] of all of it
//! for the interface and the notification.
//!
//! N4N0 stays silent here: nothing ever goes to VRChat's chatbox, its assistant and AFK message are off, and it does not
//! listen (no microphone in this version).

use crate::connection::{self, Phase, Signals};
use crate::phone::{Advertised, Phone};
use crate::prefs::Prefs;
use orbiters_core::account::{self, Api, Callback, Check, Exchange, Server};
use orbiters_core::osc::{
    self, Discovery, LanVrchat, Latest, Network, OscEvent, OscService, OscValue, Ports, Sighting, AVATAR_CHANGE, PARAMETERS_PREFIX, VRCHAT_OSC_PORT,
};
use orbiters_core::settings::Settings;
use orbiters_n4n0::status::{OrbStatus, StateAction};
use orbiters_n4n0::{Actions, Command, Files, Host, N4n0, NoticeKind, Osc, Scene, Shared};
use orbiters_orb::text::Message;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

/// What differs between the phone apps, beside what [`Phone`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The name the OSC services are announced with (VRChat's OSC debug lists it).
    pub service_name: &'static str,
    /// The tool name Orbiters knows the app by (the server's editor-link clients).
    pub client: &'static str,
    /// The app's own file in the settings' folder ([`Prefs`]).
    pub prefs_file: &'static str,
    /// How the app and VRChat find each other.
    pub discovery: Discovery,
    /// The OSC services keep their ports from one start to the next: an app the system suspends has to start them again
    /// while VRChat, which only sends to the ports it found, goes on.
    pub keep_ports: bool,
}

/// An app away for less than this was not suspended (a phone gives a moment in the background): nothing to catch up on.
const AWAY_UNNOTICED: Duration = Duration::from_secs(20);
/// How long a return waits for VRChat's OSCQuery service to be known again, before reading what it wears.
const CATCH_UP_WAIT: Duration = Duration::from_secs(10);
/// VRChat counts as there while its OSCQuery service answered, or it sent something, this recently.
const ANSWERED_FOR: Duration = Duration::from_secs(15);
const HEARD_FOR: Duration = Duration::from_secs(30);
/// The phone's local address must stay missing this long before the services stop (a list of interfaces that comes
/// back empty once is no lost Wi-Fi).
const NETWORK_GONE_AFTER: Duration = Duration::from_secs(10);
/// How often VRChat's OSCQuery service is asked whether it is still there.
const ASK_EVERY: Duration = Duration::from_secs(5);

/// The link with VRChat, as the interface shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub phase: Phase,
    /// The phone's address on the local network, and its interface (`wlan0`).
    pub phone: Option<(String, Ipv4Addr)>,
    /// Where VRChat is (found, or typed).
    pub quest: Option<Ipv4Addr>,
    /// What VRChat's OSCQuery service calls itself (`VRChat-Client-ABC123`).
    pub quest_name: Option<String>,
    /// The Quest's address was typed (no discovery).
    pub typed: bool,
    /// The port VRChat sends to on the phone.
    pub osc_port: Option<u16>,
    /// Why the services could not start.
    pub error: Option<String>,
}

/// The Orbiters account, as the interface shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountState {
    /// The stored credential is being checked.
    #[default]
    Checking,
    SignedOut,
    /// The browser is out signing in.
    SigningIn,
    SignedIn {
        name: String,
    },
}

/// A message for the person (a toast), numbered so each shows even when it says the same as the last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// Everything the interface and the notification show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub link: Link,
    pub orb: OrbStatus,
    pub account: AccountState,
    pub account_error: Option<String>,
    /// GoGoLoco's pose and the avatar's size follow to the next avatar (N4N0's carried parameters).
    pub carry: bool,
    /// The Quest's address as typed in Settings.
    pub quest_typed: Option<Ipv4Addr>,
    /// Android restricts the app's battery use (`None`: cannot tell).
    pub battery_unrestricted: Option<bool>,
    pub notice: Option<Notice>,
}

/// What the interface asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    States(StateAction),
    /// Talk with the VRChat at this address from now on (`None`: find it on the Wi-Fi).
    QuestAddress(Option<Ipv4Addr>),
    /// Announce the services again, for a VRChat that started before them.
    LookAgain,
    SetCarry(bool),
    SignIn,
    CancelSignIn,
    SignOut,
    OpenBatterySettings,
    Quit,
}

/// The OSC services running, and for which network and Quest.
struct Running {
    service: OscService,
    key: (Ipv4Addr, Option<Ipv4Addr>),
    interface: String,
}

/// What the link thread knows of VRChat.
#[derive(Default)]
struct Watch {
    answered: Option<Instant>,
    asked: Option<Instant>,
    name: Option<String>,
    error: Option<String>,
    /// The key the services last failed to start for (not tried again until it changes or Look again).
    failed_for: Option<(Ipv4Addr, Option<Ipv4Addr>)>,
    /// Since when the phone's local address has been missing while the services ran (Android's list of interfaces
    /// comes back empty now and then: the services stop only once it stays so).
    missing_since: Option<Instant>,
    /// When Android's battery restriction was last read (the person may change it in Android's settings).
    battery_read: Option<Instant>,
    /// What the system's Bonjour says of VRChat's services ([`Discovery::Platform`]), kept for the next start of the
    /// services: it may not say it again.
    seen: Vec<Sighting>,
    /// When the app went to the background, while it is there.
    away_since: Option<SystemTime>,
}

/// A sign-in under way.
struct SignIn {
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct AccountRuntime {
    generation: u64,
    signing_in: Option<SignIn>,
    account: Option<account::Account>,
}

pub struct Core {
    config: Config,
    phone: Arc<dyn Phone>,
    settings: Mutex<Settings>,
    settings_file: PathBuf,
    prefs: Mutex<Prefs>,
    prefs_file: PathBuf,
    osc: Mutex<Option<Running>>,
    latest: Mutex<Latest>,
    watch: Mutex<Watch>,
    snapshot: Mutex<Snapshot>,
    listeners: Mutex<Vec<(u64, Arc<dyn Fn() + Send + Sync>)>>,
    next_listener: AtomicU64,
    /// What OSC messages ask N4N0 for, in order, off the receiving thread.
    osc_actions: Actions,
    /// What the interface asks for, in order, off its thread.
    ui_actions: Actions,
    n4n0: OnceLock<Shared>,
    account: Mutex<AccountRuntime>,
    notices: AtomicU64,
    stop: AtomicBool,
    /// VRChat was connected when the app went away: until the app has caught up, N4N0 goes on seeing it there. An app
    /// thawed by the system finds every "lately" long past and the link down, which says nothing of VRChat, and N4N0
    /// forgets an avatar whose VRChat went (what would follow to the next avatar, first of all).
    holding_vrchat: AtomicBool,
    /// The services are being stopped to start again on the same ports: the link thread waits, as their sockets are
    /// still bound until they have stopped.
    restarting: AtomicBool,
    /// Wakes the link thread before its next round (the Quest's address changed, Look again).
    wake: (Mutex<bool>, Condvar),
    /// The server the account is for (Unity's switch does not exist here: production).
    server: Server,
}

static CORE: OnceLock<Arc<Core>> = OnceLock::new();

/// Force-silent N4N0 settings: no assistant, no AFK message on the panel, no wake word, no calls. The phone app has no
/// microphone and N4N0 never speaks into VRChat's chatbox; these keep whatever an older file said from turning any on.
pub fn silenced(mut s: Settings) -> Settings {
    s.orb.assistant = false;
    s.orb.wake_word = false;
    s.orb.afk_message_enabled = false;
    s.orb.calls = false;
    s.orb.menu.wake_word = false;
    s.world_performance = false;
    s.walk_boost.enabled = false;
    s.osc.enabled = true;
    s
}

impl Core {
    /// The process's core, started on first use with `phone`.
    pub fn get_or_start(config: Config, phone: impl FnOnce() -> Arc<dyn Phone>) -> Arc<Core> {
        CORE.get_or_init(|| Core::start(config, phone(), orbiters_core::paths::settings_file(), Prefs::file(config.prefs_file), Files::app(), true)).clone()
    }

    /// The process's core, when started.
    pub fn get() -> Option<Arc<Core>> {
        CORE.get().cloned()
    }

    /// A core keeping its settings and N4N0's files where it is told; `run` starts OSC, N4N0 and the account check
    /// (tests and screenshots leave it off).
    pub fn start(config: Config, phone: Arc<dyn Phone>, settings_file: PathBuf, prefs_file: PathBuf, files: Files, run: bool) -> Arc<Core> {
        let settings = silenced(Settings::load_from(&settings_file));
        let prefs = Prefs::load_from(&prefs_file);
        let carry = !settings.orb.carry_parameters.is_empty();
        let core = Arc::new(Core {
            config,
            phone: phone.clone(),
            settings: Mutex::new(settings),
            settings_file,
            prefs: Mutex::new(prefs.clone()),
            prefs_file,
            osc: Mutex::new(None),
            latest: Mutex::new(Latest::default()),
            watch: Mutex::new(Watch::default()),
            snapshot: Mutex::new(Snapshot { carry, quest_typed: prefs.quest, ..Default::default() }),
            listeners: Mutex::new(Vec::new()),
            next_listener: AtomicU64::new(1),
            osc_actions: Actions::default(),
            ui_actions: Actions::default(),
            n4n0: OnceLock::new(),
            account: Mutex::new(AccountRuntime::default()),
            notices: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            holding_vrchat: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            server: Server::Production,
        });
        let bridge = Arc::new(Bridge(Arc::downgrade(&core)));
        let n = N4n0::new(bridge.clone(), bridge, crate::phone::platform(phone), files);
        let _ = core.n4n0.set(n);
        if run {
            core.n4n0().start();
            let c = core.clone();
            let _ = std::thread::Builder::new().name("link".into()).spawn(move || c.link_loop());
            let c = core.clone();
            let _ = std::thread::Builder::new().name("account-check".into()).spawn(move || c.check_account());
        }
        core
    }

    pub fn n4n0(&self) -> &Shared {
        self.n4n0.get().expect("N4N0 is made with the core")
    }

    /// Calls `f` after every change of the snapshot (on the thread that changed it), until [`Core::unsubscribe`].
    pub fn subscribe(&self, f: impl Fn() + Send + Sync + 'static) -> u64 {
        let key = self.next_listener.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().unwrap().push((key, Arc::new(f)));
        key
    }

    /// A window that closed stops listening.
    pub fn unsubscribe(&self, key: u64) {
        self.listeners.lock().unwrap().retain(|(k, _)| *k != key);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Changes the snapshot; listeners hear of it when it changed.
    pub fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        let changed = {
            let mut s = self.snapshot.lock().unwrap();
            let before = s.clone();
            f(&mut s);
            *s != before
        };
        if changed {
            let listeners: Vec<_> = self.listeners.lock().unwrap().iter().map(|(_, l)| l.clone()).collect();
            for l in listeners {
                l();
            }
        }
    }

    fn notify(&self, text: impl Into<String>, error: bool) {
        let id = self.notices.fetch_add(1, Ordering::Relaxed) + 1;
        let text = text.into();
        self.update(|s| s.notice = Some(Notice { id, text, error }));
    }

    /// One line for the notification: how N4N0 stands with the Quest.
    pub fn status_line(&self) -> String {
        crate::format::notification_line(&self.snapshot())
    }

    /// What the interface asks for, done in order off its thread.
    pub fn act(self: &Arc<Self>, action: Action) {
        let c = self.clone();
        self.ui_actions.run(Box::new(move || c.act_now(action)));
    }

    fn act_now(self: &Arc<Self>, action: Action) {
        match action {
            Action::States(action) => {
                if self.snapshot().link.phase != Phase::Connected && matches!(action, StateAction::Save { .. } | StateAction::Apply { .. }) {
                    self.notify("Connect to VRChat on the Quest first.", true);
                    return;
                }
                self.n4n0().command(Command::States { action });
            }
            Action::QuestAddress(quest) => {
                {
                    let mut prefs = self.prefs.lock().unwrap();
                    prefs.quest = quest;
                    if let Err(e) = prefs.save_to(&self.prefs_file) {
                        eprintln!("the app's own settings: {e:#}");
                    }
                }
                self.watch.lock().unwrap().failed_for = None;
                self.update(|s| s.quest_typed = quest);
                self.wake_link();
            }
            Action::LookAgain => {
                self.watch.lock().unwrap().failed_for = None;
                if let Some(r) = self.osc.lock().unwrap().as_ref() {
                    r.service.rediscover();
                }
                if self.config.discovery == Discovery::Platform {
                    self.phone.browse(false);
                    self.phone.browse(true);
                }
                self.wake_link();
                self.notify("Announced again: VRChat should find N4N0 within a few seconds.", false);
            }
            Action::SetCarry(on) => {
                self.change_settings(|s| {
                    s.orb.carry_parameters = if on { orbiters_core::settings::OrbSettings::default().carry_parameters } else { Vec::new() }
                });
                self.update(|s| s.carry = on);
            }
            Action::SignIn => {
                if let Err(e) = self.sign_in() {
                    self.update(|s| s.account_error = Some(format!("{e:#}")));
                    self.notify(format!("Could not start signing in: {e:#}"), true);
                }
            }
            Action::CancelSignIn => self.cancel_sign_in(),
            Action::SignOut => self.sign_out(),
            Action::OpenBatterySettings => {
                if let Err(e) = self.phone.open_battery_settings() {
                    self.notify(format!("{e:#}"), true);
                }
            }
            Action::Quit => self.phone.quit(),
        }
    }

    fn change_settings(&self, f: impl FnOnce(&mut Settings)) {
        let snapshot = {
            let mut s = self.settings.lock().unwrap();
            f(&mut s);
            *s = silenced(s.clone());
            s.clone()
        };
        if let Err(e) = snapshot.save_to(&self.settings_file) {
            eprintln!("settings: {e:#}");
        }
    }

    fn wake_link(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Stops everything: N4N0 (its screen closed while OSC still runs), then OSC and the sign-in.
    pub fn shutdown(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        self.n4n0().shutdown();
        if let Some(s) = self.account.lock().unwrap().signing_in.take() {
            s.stop.store(true, Ordering::Relaxed);
        }
        let running = self.osc.lock().unwrap().take();
        drop(running);
        self.stop_system_discovery();
        self.wake_link();
    }

    // ---- The app coming and going (a phone that suspends apps) ----

    /// The app went to the background. On a phone that suspends it, nothing runs from a moment later on.
    pub fn went_away(&self) {
        if self.snapshot().link.phase == Phase::Connected {
            self.holding_vrchat.store(true, Ordering::SeqCst);
        }
        self.watch.lock().unwrap().away_since = Some(SystemTime::now());
    }

    /// The app is back. After a real absence, the services start again on the same ports (the system may have taken their
    /// sockets back) and what VRChat did meanwhile is caught up with: an avatar switch comes through N4N0 as if heard now,
    /// so what follows the person and the avatar's default state still go.
    pub fn came_back(self: &Arc<Self>) {
        let since = self.watch.lock().unwrap().away_since.take();
        let away = since.and_then(|t| t.elapsed().ok()).unwrap_or_default();
        if away < AWAY_UNNOTICED {
            self.holding_vrchat.store(false, Ordering::SeqCst);
            return;
        }
        let c = self.clone();
        self.ui_actions.run(Box::new(move || c.catch_up(away)));
    }

    fn catch_up(self: &Arc<Self>, away: Duration) {
        eprintln!("Back after {} s away: the OSC services start again", away.as_secs());
        let was_connected = self.holding_vrchat.load(Ordering::SeqCst);
        self.restarting.store(true, Ordering::SeqCst);
        let running = self.osc.lock().unwrap().take();
        if let Some(running) = running {
            running.service.stop_quietly();
        }
        self.forget_vrchat();
        self.watch.lock().unwrap().failed_for = None;
        self.restarting.store(false, Ordering::SeqCst);
        self.wake_link();
        if was_connected {
            self.notify(paused_note(away), false);
        }
        let deadline = Instant::now() + CATCH_UP_WAIT;
        let addr = loop {
            if let Some(addr) = self.osc.lock().unwrap().as_ref().and_then(|r| r.service.vrchat_query()) {
                break Some(addr);
            }
            if Instant::now() >= deadline || self.stop.load(Ordering::Relaxed) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if let Some(addr) = addr {
            self.catch_up_avatar(addr);
            // VRChat answered: the link says so within a round or two, before N4N0 looks again.
            let deadline = Instant::now() + CATCH_UP_WAIT;
            while self.snapshot().link.phase != Phase::Connected && Instant::now() < deadline && !self.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        // From here on the link says whether VRChat is there. If it went meanwhile, N4N0 now forgets its avatar.
        self.holding_vrchat.store(false, Ordering::SeqCst);
    }

    /// What VRChat wears now, from its OSCQuery service: a different avatar than N4N0 last heard of is a switch it missed.
    fn catch_up_avatar(&self, addr: SocketAddr) {
        let values = match osc::vrchat_values(addr, Duration::from_secs(4)) {
            Ok(values) => values,
            Err(e) => {
                eprintln!("Catching up with VRChat: {e:#}");
                return;
            }
        };
        let worn = values.into_iter().find(|(a, _)| a == AVATAR_CHANGE).and_then(|(_, v)| match v {
            OscValue::Text(id) => Some(id),
            _ => None,
        });
        let known = self.snapshot().orb.avatar_id;
        if let Some(id) = worn.filter(|id| known.as_ref() != Some(id)) {
            eprintln!("VRChat changed avatar meanwhile: {id}");
            // As when VRChat says so (`on_osc_events`): N4N0 reads what follows, then those values go.
            self.n4n0().on_avatar(id.clone());
            self.latest.lock().unwrap().record(AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)], orbiters_core::clock::now_ns());
        }
    }

    /// What the system's Bonjour saw of VRChat's OSCQuery service ([`Discovery::Platform`]).
    pub fn saw(&self, sighting: Sighting) {
        {
            let mut w = self.watch.lock().unwrap();
            let name = match &sighting {
                Sighting::Found { name, .. } | Sighting::Gone { name } => name.clone(),
            };
            w.seen.retain(|s| !matches!(s, Sighting::Found { name: n, .. } if *n == name));
            if matches!(sighting, Sighting::Found { .. }) {
                w.seen.push(sighting.clone());
            }
        }
        if let Some(r) = self.osc.lock().unwrap().as_ref() {
            r.service.saw(sighting);
        }
    }

    fn stop_system_discovery(&self) {
        if self.config.discovery == Discovery::Platform {
            self.phone.advertise(None);
            self.phone.browse(false);
        }
    }

    // ---- OSC with VRChat ----

    /// Every second: the network the services listen on (started again when the Wi-Fi changes or the Quest's address
    /// is typed), whether VRChat still answers, and the link as the snapshot shows it.
    fn link_loop(self: Arc<Self>) {
        while !self.stop.load(Ordering::Relaxed) {
            self.link_round(Instant::now());
            let (lock, cv) = &self.wake;
            let mut woken = lock.lock().unwrap();
            if !*woken {
                woken = cv.wait_timeout(woken, Duration::from_secs(1)).unwrap().0;
            }
            *woken = false;
        }
    }

    fn link_round(self: &Arc<Self>, now: Instant) {
        if self.restarting.load(Ordering::SeqCst) {
            return;
        }
        let mut phone = connection::choose_interface(&osc::lan_addresses());
        let quest = self.prefs.lock().unwrap().quest;
        let running = self.osc.lock().unwrap().as_ref().map(|r| (r.interface.clone(), r.key.0));
        {
            let mut w = self.watch.lock().unwrap();
            match (&phone, running) {
                (None, Some(kept)) if w.missing_since.get_or_insert(now).elapsed() < NETWORK_GONE_AFTER => phone = Some(kept),
                (None, _) => {}
                (Some(_), _) => w.missing_since = None,
            }
        }
        let want = phone.as_ref().map(|(_, ip)| (*ip, quest));
        let have = self.osc.lock().unwrap().as_ref().map(|r| r.key);
        if want != have {
            eprintln!(
                "OSC: network {} (was {})",
                want.map_or("none".into(), |(ip, q)| format!("{ip}, Quest {q:?}")),
                have.map_or("none".into(), |(ip, q)| format!("{ip}, Quest {q:?}"))
            );
            // The services before go outside the lock: stopping waits for their receiving thread.
            let before = self.osc.lock().unwrap().take();
            if before.is_some() {
                self.stop_system_discovery();
            }
            drop(before);
            self.forget_vrchat();
            if let (Some((name, ip)), Some(key)) = (phone.clone(), want) {
                if self.watch.lock().unwrap().failed_for != Some(key) {
                    self.start_osc(name, ip, quest, key);
                }
            }
        }
        self.ask_vrchat(now);
        let link = self.link(phone, quest, now);
        let battery = {
            let mut w = self.watch.lock().unwrap();
            let due = w.battery_read.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5));
            if due {
                w.battery_read = Some(now);
            }
            due
        };
        let battery = battery.then(|| self.phone.battery_unrestricted());
        self.update(|s| {
            s.link = link;
            if let Some(b) = battery {
                s.battery_unrestricted = b;
            }
        });
    }

    fn start_osc(self: &Arc<Self>, interface: String, ip: Ipv4Addr, quest: Option<Ipv4Addr>, key: (Ipv4Addr, Option<Ipv4Addr>)) {
        let network = Network::Lan { ip, vrchat: LanVrchat { ip: quest, name: None, osc_port: VRCHAT_OSC_PORT } };
        let weak = Arc::downgrade(self);
        let ports = match self.config.keep_ports.then(|| self.prefs.lock().unwrap().ports).flatten() {
            Some([osc, query]) => Ports { osc, query },
            None => Ports::default(),
        };
        let started = OscService::start_with(self.config.service_name, &[], network, self.config.discovery, ports, move |events, _at| {
            if let Some(core) = weak.upgrade() {
                core.on_osc_events(events);
            }
        });
        match started {
            Ok(service) => {
                eprintln!(
                    "OSC: listening on {}:{} ({interface}), VRChat {}",
                    ip,
                    service.osc_port,
                    quest.map_or("found on the Wi-Fi".into(), |q| q.to_string())
                );
                let mut w = self.watch.lock().unwrap();
                w.error = None;
                w.failed_for = None;
                let seen = w.seen.clone();
                drop(w);
                if self.config.keep_ports {
                    self.keep_ports([service.osc_port, service.query_port]);
                }
                if self.config.discovery == Discovery::Platform {
                    self.phone.advertise(Some(Advertised {
                        instance: format!("{}-{}", self.config.service_name.replace(' ', "-"), service.osc_port),
                        osc_port: service.osc_port,
                        query_port: service.query_port,
                    }));
                    self.phone.browse(true);
                    for sighting in seen {
                        service.saw(sighting);
                    }
                }
                *self.osc.lock().unwrap() = Some(Running { service, key, interface });
            }
            Err(e) => {
                eprintln!("OSC could not start on {ip}: {e:#}");
                let mut w = self.watch.lock().unwrap();
                w.error = Some(format!("{e:#}"));
                w.failed_for = Some(key);
            }
        }
    }

    fn keep_ports(&self, ports: [u16; 2]) {
        let mut prefs = self.prefs.lock().unwrap();
        if prefs.ports != Some(ports) {
            prefs.ports = Some(ports);
            if let Err(e) = prefs.save_to(&self.prefs_file) {
                eprintln!("the app's own settings: {e:#}");
            }
        }
    }

    /// What VRChat said, on the OSC receiving thread: N4N0 hears the avatar's parameters and changes, and the values
    /// are kept (N4N0 reads the last avatar's before they go).
    fn on_osc_events(&self, events: &mut Vec<OscEvent>) {
        let n = self.n4n0();
        let mut kept = Vec::with_capacity(events.len());
        for event in events.drain(..) {
            match event {
                OscEvent::Parameter { name, value } => {
                    // Face tracking's flood skips N4N0 (none of its parameters).
                    if !(name.starts_with("v2/") || name.starts_with("FT/")) {
                        n.on_parameter(&name, &value);
                    }
                    kept.push((format!("{PARAMETERS_PREFIX}{name}"), vec![value]));
                }
                OscEvent::AvatarChanged { id } => {
                    n.on_avatar(id.clone());
                    kept.push((AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)]));
                }
                OscEvent::Values { address, values } => kept.push((address, values)),
            }
        }
        let at = orbiters_core::clock::now_ns();
        let mut latest = self.latest.lock().unwrap();
        for (address, values) in kept {
            latest.record(address, values, at);
        }
    }

    /// Asks VRChat's OSCQuery service (when found) whether it is still there, every few seconds, outside every lock.
    fn ask_vrchat(&self, now: Instant) {
        let query = self.osc.lock().unwrap().as_ref().and_then(|r| r.service.vrchat_query());
        let Some(addr) = query else { return };
        {
            let mut w = self.watch.lock().unwrap();
            if w.asked.is_some_and(|t| now.duration_since(t) < ASK_EVERY) {
                return;
            }
            w.asked = Some(now);
        }
        let answer = osc::http_get(addr, "/?HOST_INFO", Duration::from_secs(2)).ok().and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok());
        let mut w = self.watch.lock().unwrap();
        if let Some(info) = answer {
            w.answered = Some(Instant::now());
            w.name = info["NAME"].as_str().map(str::to_string);
        }
    }

    /// The services stopped or moved: what was known of VRChat goes.
    fn forget_vrchat(&self) {
        let mut w = self.watch.lock().unwrap();
        w.answered = None;
        w.asked = None;
        w.name = None;
    }

    fn link(&self, phone: Option<(String, Ipv4Addr)>, typed: Option<Ipv4Addr>, now: Instant) -> Link {
        let osc = self.osc.lock().unwrap();
        let running = osc.as_ref();
        let target = running.and_then(|r| r.service.vrchat_target());
        let heard_ns = running.and_then(|r| r.service.last_received_ns());
        let osc_port = running.map(|r| r.service.osc_port);
        let interface = running.map(|r| r.interface.clone());
        drop(osc);
        let heard_ago = heard_ns.map(|t| orbiters_core::clock::now_ns().saturating_sub(t) / 1_000_000_000);
        let w = self.watch.lock().unwrap();
        let answered = w.answered.is_some_and(|t| now.duration_since(t) < ANSWERED_FOR);
        let signals = Signals {
            wifi: phone.is_some(),
            running: osc_port.is_some(),
            failed: w.error.is_some() && osc_port.is_none(),
            target: target.is_some(),
            answered,
            heard: heard_ago.is_some_and(|s| s < HEARD_FOR.as_secs()),
        };
        let quest = target.and_then(|t| match t {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        });
        Link {
            phase: connection::phase(signals),
            phone: phone.map(|(name, ip)| (interface.unwrap_or(name), ip)),
            quest: quest.or(typed),
            quest_name: w.name.clone(),
            typed: typed.is_some(),
            osc_port,
            error: w.error.clone(),
        }
    }

    // ---- The Orbiters account ----

    /// Reads the stored credential and asks Orbiters whether it still works.
    fn check_account(self: Arc<Self>) {
        let Some(stored) = account::read(self.server) else {
            self.update(|s| s.account = AccountState::SignedOut);
            return;
        };
        let name = stored.username.clone().unwrap_or_else(|| "your account".into());
        self.account.lock().unwrap().account = Some(stored.clone());
        self.update(|s| s.account = AccountState::SignedIn { name: name.clone() });
        match Api::new(self.server).check(&stored.token) {
            Ok(Check::Valid(fresh)) => {
                let name = fresh.username.clone().unwrap_or(name);
                self.account.lock().unwrap().account = Some(fresh);
                self.update(|s| s.account = AccountState::SignedIn { name });
            }
            Ok(Check::Refused) => {
                let _ = account::remove(self.server);
                self.account.lock().unwrap().account = None;
                self.update(|s| {
                    s.account = AccountState::SignedOut;
                    s.account_error = Some("Orbiters signed this phone out. Sign in again.".into());
                });
            }
            // Offline: the stored sign-in stays.
            Err(e) => eprintln!("account check: {e:#}"),
        }
    }

    fn sign_in(self: &Arc<Self>) -> anyhow::Result<()> {
        let generation = {
            let mut rt = self.account.lock().unwrap();
            rt.generation += 1;
            if let Some(s) = rt.signing_in.take() {
                s.stop.store(true, Ordering::Relaxed);
            }
            rt.generation
        };
        let before = self.snapshot().account;
        let before = if before == AccountState::SigningIn { AccountState::SignedOut } else { before };
        self.update(|s| {
            s.account = AccountState::SigningIn;
            s.account_error = None;
        });
        let api = Api::new(self.server);
        let started = (|| {
            let callback = Callback::bind()?;
            let link = crate::signin::begin_link(&api, self.config.client, &self.phone.device_name(), callback.port(), callback.path())?;
            crate::signin::check_link(self.server, &link)?;
            anyhow::Ok((callback, link))
        })();
        let (callback, link) = match started {
            Ok(started) => started,
            Err(e) => {
                if self.account.lock().unwrap().generation == generation {
                    self.update(|s| s.account = before);
                }
                return Err(e);
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut rt = self.account.lock().unwrap();
            if rt.generation != generation {
                return Ok(());
            }
            rt.signing_in = Some(SignIn { stop: stop.clone() });
        }
        let c = self.clone();
        let url = link.link_url.clone();
        std::thread::Builder::new().name("sign-in".into()).spawn(move || c.serve_sign_in(api, callback, link, generation, &stop, before))?;
        self.phone.open_sign_in(&url)
    }

    /// The listener of a sign-in, until the browser brings a code that ends it, it is cancelled, or its time runs out.
    fn serve_sign_in(self: &Arc<Self>, api: Api, callback: Callback, link: account::Link, generation: u64, stop: &AtomicBool, before: AccountState) {
        let served = callback.serve(stop, Instant::now() + crate::signin::LINK_LIFETIME, |code| {
            let exchange = api.exchange_link(&link.exchange_token, code);
            let mut rt = self.account.lock().unwrap();
            if rt.generation != generation {
                return account::Answer { location: link.failed_url(), done: true };
            }
            let answer = link.answer(&exchange);
            match exchange {
                Exchange::Done(acc) => {
                    rt.signing_in = None;
                    let saved = account::save(self.server, &acc);
                    let name = acc.username.clone().unwrap_or_else(|| "your account".into());
                    rt.account = Some(acc);
                    drop(rt);
                    self.update(|s| {
                        s.account = AccountState::SignedIn { name: name.clone() };
                        s.account_error = saved.as_ref().err().map(|e| format!("Signed in, but it could not be kept on this phone: {e:#}"));
                    });
                    self.notify(format!("Signed in as {name}."), false);
                    self.phone.close_sign_in();
                }
                Exchange::Retry(why) => {
                    drop(rt);
                    self.update(|s| s.account_error = Some(why));
                }
                Exchange::Ended(why) => {
                    rt.signing_in = None;
                    drop(rt);
                    self.update(|s| {
                        s.account = before.clone();
                        s.account_error = Some(why);
                    });
                    self.phone.close_sign_in();
                }
            }
            answer
        });
        if served != account::Served::Done {
            let mut rt = self.account.lock().unwrap();
            if rt.generation == generation {
                rt.signing_in = None;
                drop(rt);
                let timed_out = served == account::Served::TimedOut;
                self.update(|s| {
                    s.account = before.clone();
                    if timed_out {
                        s.account_error = Some("The sign-in link expired. Sign in again.".into());
                    }
                });
                self.phone.close_sign_in();
            }
        }
    }

    fn cancel_sign_in(&self) {
        let mut rt = self.account.lock().unwrap();
        rt.generation += 1;
        if let Some(s) = rt.signing_in.take() {
            s.stop.store(true, Ordering::Relaxed);
        }
        let signed_in = rt.account.as_ref().map(|a| a.username.clone().unwrap_or_else(|| "your account".into()));
        drop(rt);
        self.update(|s| {
            s.account = match signed_in {
                Some(name) => AccountState::SignedIn { name },
                None => AccountState::SignedOut,
            };
            s.account_error = None;
        });
        self.phone.close_sign_in();
    }

    fn sign_out(&self) {
        self.cancel_sign_in();
        let removed = account::remove(self.server);
        self.account.lock().unwrap().account = None;
        self.update(|s| {
            s.account = AccountState::SignedOut;
            s.account_error = removed.err().map(|e| format!("{e:#}"));
        });
    }

    fn account_api(&self) -> Option<(Api, String)> {
        let rt = self.account.lock().unwrap();
        rt.account.as_ref().map(|a| (Api::new(self.server), a.token.clone()))
    }
}

/// What a person back from an absence is told.
fn paused_note(away: Duration) -> String {
    let minutes = away.as_secs() / 60;
    let time = match minutes {
        0 => "under a minute".to_string(),
        1 => "1 minute".to_string(),
        m if m < 120 => format!("{m} minutes"),
        m => format!("{} hours", m / 60),
    };
    format!("N4N0 was paused for {time} while the phone was away from it. It is catching up with VRChat.")
}

/// The core as N4N0's host and OSC, held weakly: N4N0 lives inside it.
struct Bridge(Weak<Core>);

impl Bridge {
    fn with<R>(&self, f: impl FnOnce(&Arc<Core>) -> R) -> Option<R> {
        self.0.upgrade().map(|c| f(&c))
    }
}

impl Host for Bridge {
    fn with_settings(&self, read: &mut dyn FnMut(&Settings)) {
        match self.0.upgrade() {
            Some(c) => read(&c.settings.lock().unwrap()),
            None => read(&silenced(Settings::default())),
        }
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) {
        self.with(|c| {
            c.change_settings(|s| change(s));
            let carry = !c.settings.lock().unwrap().orb.carry_parameters.is_empty();
            c.update(|s| s.carry = carry);
        });
    }

    /// VRChat counts as running while it answers on the Wi-Fi; on a Quest it always runs in VR.
    fn scene(&self) -> Scene {
        self.with(|c| Scene {
            vrchat: c.snapshot.lock().unwrap().link.phase == Phase::Connected || c.holding_vrchat.load(Ordering::SeqCst),
            vrchat_pid: None,
            steamvr: true,
        })
        .unwrap_or_default()
    }

    fn publish(&self, status: OrbStatus) {
        self.with(|c| c.update(|s| s.orb = status));
    }

    fn notify(&self, kind: NoticeKind, message: String) {
        self.with(|c| c.notify(message, matches!(kind, NoticeKind::Error | NoticeKind::Warning)));
    }

    fn account(&self) -> Option<(Api, String)> {
        self.with(|c| c.account_api()).flatten()
    }

    fn queue(&self, action: Box<dyn FnOnce() + Send>) -> bool {
        self.with(|c| c.osc_actions.run(action)).unwrap_or(false)
    }
}

impl Osc for Bridge {
    fn send(&self, bundles: &[&[Message]]) -> Option<Vec<anyhow::Result<()>>> {
        self.with(|c| {
            let osc = c.osc.lock().unwrap();
            let service = &osc.as_ref()?.service;
            Some(
                bundles
                    .iter()
                    .map(|bundle| match bundle {
                        [(address, arg)] => service.send(address, vec![arg.clone()]),
                        _ => service.send_bundle(bundle.iter().map(|(address, arg)| (address.clone(), vec![arg.clone()])).collect()),
                    })
                    .collect(),
            )
        })
        .flatten()
    }

    fn vrchat_query(&self) -> Option<Option<SocketAddr>> {
        self.with(|c| c.osc.lock().unwrap().as_ref().map(|r| r.service.vrchat_query())).flatten()
    }

    fn latest(&self, addresses: &mut dyn Iterator<Item = String>) -> HashMap<String, OscValue> {
        self.with(|c| {
            let latest = c.latest.lock().unwrap();
            addresses.filter_map(|a| latest.first(&a).map(|v| (a, v))).collect()
        })
        .unwrap_or_default()
    }

    /// VRChat's tree over the Wi-Fi (a big avatar's is a few hundred kilobytes: a few seconds at most).
    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        let Some(addr) = self.vrchat_query().flatten() else { return Vec::new() };
        osc::vrchat_values(addr, Duration::from_secs(4)).unwrap_or_else(|e| {
            eprintln!("VRChat's values: {e:#}");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone::NoPhone;

    /// The address of a stand-in Quest: 127.0.0.2 where all of 127/8 is loopback (Windows, Linux), else 127.0.0.1 (a Mac
    /// without `ifconfig lo0 alias 127.0.0.2`), told apart from the services by its port.
    fn stand_in_quest() -> Ipv4Addr {
        let other = Ipv4Addr::new(127, 0, 0, 2);
        if std::net::UdpSocket::bind((other, 0)).is_ok() {
            other
        } else {
            Ipv4Addr::LOCALHOST
        }
    }

    const CONFIG: Config =
        Config { service_name: "Orbiters - N4N0 Test", client: "test", prefs_file: "phone.json", discovery: Discovery::Mdns, keep_ports: false };

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("n4n0-mobile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn core_in(d: &std::path::Path) -> Arc<Core> {
        Core::start(CONFIG, Arc::new(NoPhone), d.join("settings.json"), d.join("phone.json"), Files::in_dir(d), false)
    }

    #[test]
    fn n4n0_stays_silent_whatever_the_file_says() {
        let d = dir("silent");
        let mut loud = Settings::default();
        loud.orb.assistant = true;
        loud.orb.afk_message_enabled = true;
        loud.orb.wake_word = true;
        loud.orb.calls = true;
        loud.save_to(&d.join("settings.json")).unwrap();
        let core = core_in(&d);
        let s = core.settings.lock().unwrap().clone();
        assert!(!s.orb.assistant && !s.orb.afk_message_enabled && !s.orb.wake_word && !s.orb.calls);
        assert!(s.orb.enabled, "N4N0's helpers (carried parameters, states) still run");
        // N4N0 asking to change them cannot turn them back on.
        Bridge(Arc::downgrade(&core)).update_settings(&mut |s| s.orb.assistant = true);
        assert!(!core.settings.lock().unwrap().orb.assistant);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn carry_and_the_quest_s_address_are_kept() {
        let d = dir("carry");
        let core = core_in(&d);
        assert!(core.snapshot().carry, "GoGoLoco's pose and size follow by default");
        core.act_now(Action::SetCarry(false));
        assert!(!core.snapshot().carry);
        assert!(Settings::load_from(&d.join("settings.json")).orb.carry_parameters.is_empty());
        core.act_now(Action::SetCarry(true));
        assert_eq!(Settings::load_from(&d.join("settings.json")).orb.carry_parameters, orbiters_core::settings::OrbSettings::default().carry_parameters);
        let quest = Ipv4Addr::new(192, 168, 1, 42);
        core.act_now(Action::QuestAddress(Some(quest)));
        assert_eq!(core.snapshot().quest_typed, Some(quest));
        assert_eq!(Prefs::load_from(&d.join("phone.json")).quest, Some(quest));
        drop(core);
        let again = core_in(&d);
        assert_eq!(again.snapshot().quest_typed, Some(quest), "read back at the next start");
        assert!(again.snapshot().carry);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn states_wait_for_vrchat() {
        let d = dir("states");
        let core = core_in(&d);
        core.act_now(Action::States(StateAction::Save { name: Some("Dance".into()) }));
        let notice = core.snapshot().notice.expect("told why nothing happened");
        assert!(notice.error && notice.text.contains("Connect"), "{notice:?}");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A stand-in for VRChat's OSCQuery service on this computer: `/avatar` answers `tree` as it is at that moment,
    /// anything else its HOST_INFO.
    fn fake_query(tree: Arc<Mutex<serde_json::Value>>) -> SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = if path.starts_with("/avatar") {
                    tree.lock().unwrap().to_string()
                } else {
                    serde_json::json!({ "NAME": "VRChat-Client-TEST", "OSC_PORT": 9000, "OSC_IP": "127.0.0.2" }).to_string()
                };
                let _ = stream.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                        .as_bytes(),
                );
            }
        });
        addr
    }

    fn avatar_tree(hoodie: bool) -> serde_json::Value {
        serde_json::json!({ "FULL_PATH": "/avatar", "CONTENTS": {
            "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "VALUE": ["avtr_test"], "ACCESS": 3 },
            "parameters": { "FULL_PATH": "/avatar/parameters", "CONTENTS": {
                "Hoodie": { "FULL_PATH": "/avatar/parameters/Hoodie", "TYPE": "T", "VALUE": [hoodie], "ACCESS": 3 },
                "Hue": { "FULL_PATH": "/avatar/parameters/Hue", "TYPE": "f", "VALUE": [0.25], "ACCESS": 3 }
            } }
        } })
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ok() {
            assert!(Instant::now() < deadline, "waited too long for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Saved states end to end, as the interface drives them: N4N0 learns the avatar from VRChat's tree, Save keeps
    /// how it is (in avatar-states.json), Apply sends the Quest what changed since.
    #[test]
    fn states_are_saved_and_applied_through_n4n0() {
        let d = dir("states-e2e");
        let core = core_in(&d);
        let tree = Arc::new(Mutex::new(avatar_tree(true)));
        let query = fake_query(tree.clone());
        let quest_ip = stand_in_quest();
        let quest = std::net::UdpSocket::bind((quest_ip, 0)).unwrap();
        quest.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let network =
            Network::Lan { ip: Ipv4Addr::LOCALHOST, vrchat: LanVrchat { ip: Some(quest_ip), name: None, osc_port: quest.local_addr().unwrap().port() } };
        let weak = Arc::downgrade(&core);
        let service = OscService::start_batched(CONFIG.service_name, &[], network, false, move |events, _| {
            if let Some(c) = weak.upgrade() {
                c.on_osc_events(events);
            }
        })
        .unwrap();
        service.set_vrchat_query(Some(query));
        *core.osc.lock().unwrap() = Some(Running { service, key: (Ipv4Addr::LOCALHOST, Some(quest_ip)), interface: "lo".into() });
        core.update(|s| s.link.phase = Phase::Connected);
        core.n4n0().start();
        wait_for("the avatar", || core.snapshot().orb.states.avatar_id.as_deref() == Some("avtr_test"));

        core.act_now(Action::States(StateAction::Save { name: Some("Hoodie on".into()) }));
        wait_for("the saved state", || core.snapshot().orb.states.states.iter().any(|s| s.name == "Hoodie on" && s.parameters == 2));
        let file = std::fs::read_to_string(d.join("avatar-states.json")).unwrap();
        assert!(file.contains("Hoodie on") && file.contains("avtr_test"), "{file}");
        let id = core.snapshot().orb.states.states[0].id.clone();
        core.act_now(Action::States(StateAction::Default { id: Some(id.clone()) }));
        wait_for("the default", || core.snapshot().orb.states.states[0].default);

        // The hoodie came off: applying puts it back on the Quest.
        *tree.lock().unwrap() = avatar_tree(false);
        core.act_now(Action::States(StateAction::Apply { id }));
        let mut sent = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline
            && !sent.iter().any(|(a, v): &(String, Vec<rosc::OscType>)| a == "/avatar/parameters/Hoodie" && v == &vec![rosc::OscType::Bool(true)])
        {
            if let Ok((n, _)) = quest.recv_from(&mut buf) {
                fn walk(p: rosc::OscPacket, out: &mut Vec<(String, Vec<rosc::OscType>)>) {
                    match p {
                        rosc::OscPacket::Message(m) => out.push((m.addr, m.args)),
                        rosc::OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| walk(p, out)),
                    }
                }
                walk(rosc::decoder::decode_udp(&buf[..n]).unwrap().1, &mut sent);
            }
        }
        assert!(sent.iter().any(|(a, v)| a == "/avatar/parameters/Hoodie" && v == &vec![rosc::OscType::Bool(true)]), "{sent:?}");
        assert!(!sent.iter().any(|(a, _)| a.starts_with("/chatbox")), "N4N0 never writes to the chatbox: {sent:?}");
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    /// The whole path on this computer's loopback: the services announced for a stand-in Quest, its datagrams reaching N4N0 and the values kept, N4N0's messages
    /// reaching it, and the link's phase following.
    #[test]
    fn a_quest_on_the_network_is_heard_and_answered() {
        let d = dir("lan");
        let core = core_in(&d);
        let quest_ip = stand_in_quest();
        let quest = std::net::UdpSocket::bind((quest_ip, 0)).unwrap();
        quest.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let quest_port = quest.local_addr().unwrap().port();
        let network = Network::Lan { ip: Ipv4Addr::LOCALHOST, vrchat: LanVrchat { ip: Some(quest_ip), name: None, osc_port: quest_port } };
        let weak = Arc::downgrade(&core);
        let service = OscService::start_batched(CONFIG.service_name, &[], network, false, move |events, _| {
            if let Some(c) = weak.upgrade() {
                c.on_osc_events(events);
            }
        })
        .unwrap();
        let to_phone = service.local_osc_addr();
        *core.osc.lock().unwrap() = Some(Running { service, key: (Ipv4Addr::LOCALHOST, Some(quest_ip)), interface: "lo".into() });
        let phone = Some(("lo".to_string(), Ipv4Addr::LOCALHOST));
        assert_eq!(core.link(phone.clone(), None, Instant::now()).phase, Phase::Found, "the typed Quest is known, not heard yet");
        // The Quest sends a parameter.
        let packet = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
            addr: format!("{PARAMETERS_PREFIX}GoLocoPose"),
            args: vec![rosc::OscType::Int(3)],
        }))
        .unwrap();
        quest.send_to(&packet, to_phone).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while core.latest.lock().unwrap().first(&format!("{PARAMETERS_PREFIX}GoLocoPose")).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(core.latest.lock().unwrap().first(&format!("{PARAMETERS_PREFIX}GoLocoPose")), Some(OscValue::Int(3)));
        let link = core.link(phone, None, Instant::now());
        assert_eq!(link.phase, Phase::Connected);
        assert_eq!(link.quest, Some(quest_ip));
        // N4N0's messages go to the Quest.
        let sent = Bridge(Arc::downgrade(&core)).send(&[&[("/avatar/parameters/N4N0/Visible".to_string(), rosc::OscType::Bool(true))]]);
        assert!(sent.unwrap().iter().all(|r| r.is_ok()));
        let mut buf = [0u8; 512];
        let (n, _) = quest.recv_from(&mut buf).unwrap();
        let (_, got) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        assert!(matches!(got, rosc::OscPacket::Message(m) if m.addr == "/avatar/parameters/N4N0/Visible"));
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    /// A phone whose system keeps mDNS: what the core asks of it, in order.
    #[derive(Default)]
    struct BonjourPhone {
        asked: Mutex<Vec<String>>,
    }

    impl Phone for BonjourPhone {
        fn open_url(&self, _url: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn copy(&self, _text: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn locale(&self) -> String {
            "en-US".into()
        }
        fn device_name(&self) -> String {
            "Test phone".into()
        }
        fn close_sign_in(&self) {
            self.asked.lock().unwrap().push("close sign-in".into());
        }
        fn advertise(&self, services: Option<Advertised>) {
            self.asked.lock().unwrap().push(match services {
                Some(s) => format!("advertise {} {} {}", s.instance, s.osc_port, s.query_port),
                None => "stop advertising".into(),
            });
        }
        fn browse(&self, on: bool) {
            self.asked.lock().unwrap().push(format!("browse {on}"));
        }
    }

    const IOS_LIKE: Config = Config { service_name: "N4N0 Test", client: "test", prefs_file: "phone.json", discovery: Discovery::Platform, keep_ports: true };

    fn core_with_bonjour(d: &std::path::Path) -> (Arc<Core>, Arc<BonjourPhone>) {
        let phone = Arc::new(BonjourPhone::default());
        let core = Core::start(IOS_LIKE, phone.clone(), d.join("settings.json"), d.join("phone.json"), Files::in_dir(d), false);
        (core, phone)
    }

    #[test]
    fn the_system_s_bonjour_is_told_what_to_announce_and_the_ports_are_kept() {
        let d = dir("bonjour");
        let (core, phone) = core_with_bonjour(&d);
        let key = (Ipv4Addr::LOCALHOST, None);
        core.start_osc("lo".into(), Ipv4Addr::LOCALHOST, None, key);
        let (osc, query) = {
            let running = core.osc.lock().unwrap();
            let service = &running.as_ref().unwrap().service;
            (service.osc_port, service.query_port)
        };
        assert_eq!(*phone.asked.lock().unwrap(), vec![format!("advertise N4N0-Test-{osc} {osc} {query}"), "browse true".to_string()]);
        assert_eq!(Prefs::load_from(&d.join("phone.json")).ports, Some([osc, query]));
        // Started again, with VRChat still sending to what it found: the same ports.
        let running = core.osc.lock().unwrap().take().unwrap();
        running.service.stop_quietly();
        core.start_osc("lo".into(), Ipv4Addr::LOCALHOST, None, key);
        let again = core.osc.lock().unwrap().as_ref().map(|r| (r.service.osc_port, r.service.query_port));
        assert_eq!(again, Some((osc, query)));
        core.shutdown();
        let asked = phone.asked.lock().unwrap();
        assert_eq!(asked[asked.len() - 2..], ["stop advertising".to_string(), "browse false".to_string()]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn what_bonjour_saw_is_kept_for_the_next_start_and_forgotten_when_gone() {
        let d = dir("seen");
        let (core, _phone) = core_with_bonjour(&d);
        let found = |name: &str| Sighting::Found { name: name.into(), addresses: vec![Ipv4Addr::new(192, 168, 1, 42)], port: 5000 };
        core.saw(found("VRChat-Client-A"));
        core.saw(found("VRChat-Client-A"));
        core.saw(found("VRChat-Client-B"));
        assert_eq!(core.watch.lock().unwrap().seen, vec![found("VRChat-Client-A"), found("VRChat-Client-B")]);
        core.saw(Sighting::Gone { name: "VRChat-Client-A".into() });
        assert_eq!(core.watch.lock().unwrap().seen, vec![found("VRChat-Client-B")]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_short_absence_changes_nothing_and_a_long_one_starts_the_services_again() {
        let d = dir("away");
        let (core, _phone) = core_with_bonjour(&d);
        core.start_osc("lo".into(), Ipv4Addr::LOCALHOST, None, (Ipv4Addr::LOCALHOST, None));
        core.update(|s| s.link.phase = Phase::Connected);
        core.went_away();
        core.came_back();
        std::thread::sleep(Duration::from_millis(100));
        assert!(core.osc.lock().unwrap().is_some(), "a moment away: untouched");
        assert!(core.snapshot().notice.is_none());

        core.went_away();
        core.watch.lock().unwrap().away_since = Some(SystemTime::now() - Duration::from_secs(600));
        core.came_back();
        wait_for("the note", || core.snapshot().notice.is_some());
        assert!(core.osc.lock().unwrap().is_none(), "the link thread starts them again, not left running on dead sockets");
        assert!(core.snapshot().notice.unwrap().text.contains("paused for 10 minutes"));
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    /// The link thread keeps its hands off while the services stop: their sockets are still bound, and starting now would
    /// land on other ports, where VRChat does not send.
    #[test]
    fn the_link_thread_waits_while_the_services_restart() {
        let d = dir("restart");
        let (core, _phone) = core_with_bonjour(&d);
        core.restarting.store(true, Ordering::SeqCst);
        core.link_round(Instant::now());
        assert!(core.osc.lock().unwrap().is_none(), "nothing started during a restart");
        assert_eq!(core.snapshot().link.phase, Phase::default(), "and nothing was said of the link");
        core.restarting.store(false, Ordering::SeqCst);
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    /// The phone being away is not VRChat going away: a thawed app finds its link down, and N4N0 must not forget the avatar.
    #[test]
    fn n4n0_goes_on_seeing_vrchat_through_an_absence() {
        let d = dir("hold");
        let core = core_in(&d);
        let scene = || Bridge(Arc::downgrade(&core)).scene().vrchat;
        core.update(|s| s.link.phase = Phase::Connected);
        assert!(scene());
        core.went_away();
        // Thawed: every "lately" is long past, the link round says so.
        core.update(|s| s.link.phase = Phase::Found);
        assert!(scene(), "still there for N4N0 while away");
        // A moment away and back: the link is believed again.
        core.came_back();
        assert!(!scene());

        // A long absence holds until caught up, and a VRChat that is really gone counts as gone from then on.
        core.update(|s| s.link.phase = Phase::Connected);
        core.went_away();
        core.update(|s| s.link.phase = Phase::Searching);
        core.watch.lock().unwrap().away_since = Some(SystemTime::now() - Duration::from_secs(600));
        core.came_back();
        assert!(scene(), "held while catching up");
        wait_for("the end of catching up", || !scene());
        assert!(core.snapshot().notice.is_some_and(|n| n.text.contains("paused")));

        // Away while VRChat was not connected: nothing to hold, and nothing to say.
        core.update(|s| s.notice = None);
        core.went_away();
        assert!(!scene());
        core.watch.lock().unwrap().away_since = Some(SystemTime::now() - Duration::from_secs(600));
        core.came_back();
        std::thread::sleep(Duration::from_millis(50));
        assert!(core.snapshot().notice.is_none());
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    /// An avatar switch missed while suspended comes through N4N0 as if heard: read from VRChat's tree on the way back.
    #[test]
    fn an_avatar_switch_missed_while_away_is_caught_up_with() {
        let d = dir("catch-up");
        let core = core_in(&d);
        let query = fake_query(Arc::new(Mutex::new(avatar_tree(true))));
        core.n4n0().start();
        core.update(|s| s.orb.avatar_id = None);
        core.catch_up_avatar(query);
        // `avatar_tree` wears avtr_test; N4N0's status says so once it heard of it, and the states follow.
        wait_for("the switch", || core.snapshot().orb.avatar_id.as_deref() == Some("avtr_test"));
        // Asked again: the same avatar, nothing to do (and nothing panics on an unreachable VRChat).
        core.catch_up_avatar(query);
        core.catch_up_avatar(SocketAddr::from((Ipv4Addr::LOCALHOST, 9)));
        core.shutdown();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_absence_in_words() {
        assert!(paused_note(Duration::from_secs(30)).contains("under a minute"));
        assert!(paused_note(Duration::from_secs(61)).contains("1 minute "));
        assert!(paused_note(Duration::from_secs(5400)).contains("90 minutes"));
        assert!(paused_note(Duration::from_secs(4 * 3600)).contains("4 hours"));
    }
}
