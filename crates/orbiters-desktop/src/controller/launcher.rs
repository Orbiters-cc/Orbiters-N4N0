//! N4N0 as a VRChat launcher (REQUEST10), the controller side: what Home's friends list, profile modal, invite card and
//! Play buttons read (`Snapshot::launcher`, an [`orbiters_core::launcher::LauncherState`]) and the [`Action`]s they send.
//!
//! - Signed in to VRChat in N4N0 (Settings > VRChat account): every friend (pages of 100, a second apart), the
//!   favourite friend groups, the invites, invite requests and boops waiting; then VRChat's pipeline (one websocket)
//!   keeps it all live. A full read again only when the pipeline comes back after a cut, or every 30 minutes.
//! - Otherwise: VRCNext's or VRCX's own copy (`orbiters_core::launcher_local`), read again every 2 minutes, with
//!   [`Possible`] saying that live status, invites and boops need the sign-in.
//! - Play: VRChat through Steam (`-applaunch 438100`) with the launch options of Settings; never a second VRChat: when
//!   one runs, a destination goes to it through its own link pipe (`travel`), else an invite to oneself.
//! - Play on a Mac ([`MacMachine`]): VRChat runs there through Obelisk (`orbiters_core::obelisk`), which has no way
//!   for another program to press its Play. N4N0 opens Obelisk and says to press Play there; it sees the game once
//!   it runs, and a destination is an invite to oneself (kept until the game is up when it was not).
use super::{Controller, Shared};
use orbiters_core::launcher::{Action, Game, Invite, LaunchMode, LauncherState, Notice, Place, Possible, Profile, SentKind, Source, Whereabouts};
use orbiters_core::launcher_feed::{self, Cooldowns, Decorations};
use orbiters_core::vrchat_api::{Client, Error};
use orbiters_core::vrchat_launch::{desktop_window, SavedWindow};
use orbiters_i18n::tr;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOCAL_EVERY: Duration = Duration::from_secs(120);
/// A world or decoration lookup that failed (not "not found": a time-out, VRChat unreachable) is asked again after this.
const LOOKUP_AGAIN: Duration = Duration::from_secs(10 * 60);
/// A profile's details, kept per person this long (at most this many people).
const PROFILE_FOR: Duration = Duration::from_secs(5 * 60);
const PROFILES_KEPT: usize = 64;
/// Boops put away, remembered this long (VRChat may list them again on every full read).
const DISMISSED_FOR: i64 = 30 * 24 * 3600;
/// A local copy caught busy is read again after this.
const LOCAL_RETRY: Duration = Duration::from_secs(10);
const FULL_EVERY: Duration = Duration::from_secs(30 * 60);
const GAME_EVERY: Duration = Duration::from_secs(5);
/// A start just asked for counts as "starting" this long (Steam, then Easy Anti-Cheat, then VRChat's process).
const STARTING_FOR: Duration = Duration::from_secs(60);
/// How long an action's outcome stays in the state (the page's "Sent", the error it took back).
const NOTICE_FOR: Duration = Duration::from_secs(3);
/// After a failed read of VRChat: the first try again, doubling up to the longest (with jitter), never every tick.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_LONGEST: Duration = Duration::from_secs(10 * 60);
/// A pipeline that came back asks for a full read only when the last one is older than this (else it waits until then).
const RESYNC_AFTER: Duration = Duration::from_secs(5 * 60);
/// An accepted invite is hidden at VRChat once the account is in its instance; if it never gets there in this time, the
/// card comes back (and VRChat still has the invite).
const ARRIVE_WITHIN: Duration = Duration::from_secs(10 * 60);
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);

/// The game on this computer, real or a test's.
pub trait Machine: Send + Sync {
    /// The VRChats running here: process id and whether it runs in Desktop mode (`--no-vr`).
    fn running(&self) -> Vec<(u32, bool)>;
    fn installed(&self) -> bool;
    /// Starts VRChat with these arguments (after `-applaunch 438100`).
    fn start(&self, args: &[String]) -> Result<(), String>;
    /// The desktop window VRChat saved for its next start (Unity's `Screenmanager` values): width, height, fullscreen mode.
    fn saved_window(&self) -> Option<SavedWindow> {
        None
    }
    /// A Mac without what runs VRChat there (Obelisk): nothing to install the game with yet.
    fn engine_missing(&self) -> bool {
        false
    }
    /// Whether `start` only opens what the person presses Play in (Obelisk on a Mac): the options cannot be given,
    /// a destination waits for the game, and a running game takes a destination as an invite only.
    fn by_hand(&self) -> bool {
        false
    }
    /// Opens what runs VRChat here, or where it is got (`Action::OpenEngine`).
    fn open_engine(&self) -> Result<(), String> {
        Ok(())
    }
}

/// How VRChat is started on this computer: through Steam only when Steam's library really holds it; else through the
/// launcher VRChat registers for its links (the Meta store's VRChat, or one installed outside Steam).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Steam,
    Launcher,
    NotInstalled,
}

pub fn route(steam_has_vrchat: bool, launcher_registered: bool) -> Route {
    if steam_has_vrchat {
        Route::Steam
    } else if launcher_registered {
        Route::Launcher
    } else {
        Route::NotInstalled
    }
}

#[derive(Default)]
pub struct RealMachine {
    /// Each VRChat's Desktop mode, read once from its command line.
    desktop: Mutex<HashMap<u32, bool>>,
    /// The route, kept ten minutes (it reads the registry and Steam's library file).
    route: Mutex<Option<(Instant, Route)>>,
}

const ROUTE_FOR: Duration = Duration::from_secs(10 * 60);

impl RealMachine {
    fn route_now() -> Route {
        let steam_has_vrchat = crate::bridge::win::steam_exe().and_then(|exe| exe.parent().map(|d| d.to_path_buf())).and_then(|steam| orbiters_core::vrchat_launch::vrchat_dir(&steam)).is_some();
        route(steam_has_vrchat, crate::bridge::win::vrchat_launcher().is_some())
    }

    fn route(&self, fresh: bool) -> Route {
        let mut kept = self.route.lock().unwrap();
        match *kept {
            Some((at, r)) if !fresh && at.elapsed() < ROUTE_FOR => r,
            _ => {
                let r = Self::route_now();
                *kept = Some((Instant::now(), r));
                r
            }
        }
    }
}

/// VRChat on this Mac, through Obelisk.
pub struct MacMachine {
    pub home: std::path::PathBuf,
}

impl MacMachine {
    fn app(&self) -> Option<std::path::PathBuf> {
        orbiters_core::obelisk::app_places(&self.home).into_iter().find(|p| p.is_dir())
    }
}

impl Machine for MacMachine {
    fn running(&self) -> Vec<(u32, bool)> {
        let install = orbiters_core::obelisk::vrchat_install(&orbiters_core::obelisk::library(&self.home));
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::OnlyIfNotSet));
        let mut found: Vec<u32> = system
            .processes()
            .values()
            .filter(|p| {
                let command: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
                orbiters_core::obelisk::is_vrchat_process(&command, install.as_deref())
            })
            .map(|p| p.pid().as_u32())
            .collect();
        found.sort_unstable();
        // No headset on a Mac: the desktop game, always.
        found.into_iter().map(|pid| (pid, true)).collect()
    }

    fn installed(&self) -> bool {
        orbiters_core::obelisk::engine(&self.home) == orbiters_core::obelisk::Engine::Ready
    }

    fn engine_missing(&self) -> bool {
        orbiters_core::obelisk::engine(&self.home) == orbiters_core::obelisk::Engine::Missing
    }

    fn by_hand(&self) -> bool {
        true
    }

    fn start(&self, _args: &[String]) -> Result<(), String> {
        self.open_engine()
    }

    fn open_engine(&self) -> Result<(), String> {
        let mut open = std::process::Command::new("/usr/bin/open");
        match self.app() {
            Some(app) => open.arg(app),
            // Not in Applications (or not at all): macOS finds it by its identifier, else its site.
            None if orbiters_core::obelisk::library(&self.home).is_dir() => open.args(["-b", orbiters_core::obelisk::BUNDLE_ID]),
            None => open.arg(orbiters_core::obelisk::SITE),
        };
        match open.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(tr("Obelisk could not be opened.").into()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// This computer's.
fn real_machine() -> Arc<dyn Machine> {
    if cfg!(target_os = "macos") {
        Arc::new(MacMachine { home: orbiters_core::obelisk::home() })
    } else {
        Arc::new(RealMachine::default())
    }
}

impl Machine for RealMachine {
    fn running(&self) -> Vec<(u32, bool)> {
        // Names only (no process is opened for them); the command line is read for VRChat's processes alone, once each.
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
        let pids: Vec<sysinfo::Pid> = system.processes().values().filter(|p| matches!(p.name().to_string_lossy().to_lowercase().as_str(), "vrchat.exe" | "vrchat")).map(|p| p.pid()).collect();
        let mut desktop = self.desktop.lock().unwrap();
        desktop.retain(|pid, _| pids.iter().any(|p| p.as_u32() == *pid));
        let unknown: Vec<sysinfo::Pid> = pids.iter().copied().filter(|p| !desktop.contains_key(&p.as_u32())).collect();
        if !unknown.is_empty() {
            system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&unknown), true, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always));
            for pid in unknown {
                let no_vr = system.process(pid).is_some_and(|p| p.cmd().iter().any(|a| a.to_string_lossy() == "--no-vr"));
                desktop.insert(pid.as_u32(), no_vr);
            }
        }
        pids.iter().map(|p| (p.as_u32(), desktop.get(&p.as_u32()).copied().unwrap_or(false))).collect()
    }

    fn installed(&self) -> bool {
        self.route(false) != Route::NotInstalled
    }

    fn start(&self, args: &[String]) -> Result<(), String> {
        // Through Steam as a person does when Steam has VRChat; else VRChat's own launcher with the link first (as it
        // registers itself), the options after: never reordered, `-screen-fullscreen 0` stays a pair.
        match self.route(true) {
            Route::Steam => crate::bridge::win::start_vrchat(args).map_err(|f| f.message),
            Route::Launcher => crate::bridge::win::start_vrchat_beside(args).map_err(|f| f.message),
            Route::NotInstalled => Err(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into()),
        }
    }

    fn saved_window(&self) -> Option<SavedWindow> {
        saved_window()
    }
}

/// VRChat's saved window, read from its registry key (read only; only looked at while no VRChat runs).
#[cfg(windows)]
fn saved_window() -> Option<SavedWindow> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let dword = |name: &str| {
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: a u32 and its size are passed; the names live for the call.
        let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(r"Software\VRChat\VRChat"), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size)) };
        status.is_ok().then_some(value)
    };
    Some(SavedWindow {
        width: dword("Screenmanager Resolution Width_h182942802")?, // i18n: not shown (a registry value name)
        height: dword("Screenmanager Resolution Height_h2627697771")?, // i18n: not shown (a registry value name)
        mode: dword("Screenmanager Fullscreen mode_h3630240806").unwrap_or(3), // i18n: not shown (a registry value name)
    })
}

#[cfg(not(windows))]
fn saved_window() -> Option<SavedWindow> {
    None
}

/// What the launcher keeps besides the published state.
pub(super) struct Runtime {
    /// The account the state was read for (`None`: the local databases).
    loaded_for: Option<Option<String>>,
    loaded_at: Option<Instant>,
    /// Reads of VRChat that failed in a row, and when the next try is (none: the usual clock).
    failures: u32,
    retry_at: Option<Instant>,
    favourite_groups: Vec<(String, String)>,
    cooldowns: Cooldowns,
    /// Decoration template id → picture (`None` while unknown or failed).
    pictures: HashMap<String, Option<String>>,
    /// Which friend wears which decorations.
    worn: BTreeMap<String, Decorations>,
    /// World id → its name and picture (`None` once asked and not given).
    worlds: HashMap<String, Option<(String, String)>>,
    /// World and decoration ids whose lookup failed for a passing reason, and when: asked again after [`LOOKUP_AGAIN`].
    lookup_failed: HashMap<String, Instant>,
    /// Profiles' details read, by person: `/profile`, `/users`, mutuals.
    profiles: HashMap<String, (Instant, serde_json::Value, serde_json::Value, Option<(u32, u32)>)>,
    /// Boops put away here (id → when), kept in `launcher-dismissed.json`; read once.
    dismissed: Option<BTreeMap<String, i64>>,
    pipeline: Option<Arc<AtomicBool>>,
    /// The client (one sign-in, one grant) the pipeline was opened with.
    pipeline_client: Option<Arc<Mutex<Client>>>,
    /// Invites accepted here, waiting for the account to arrive in their instance before VRChat is told to hide them.
    accepted: Vec<(Invite, Instant)>,
    /// The pipeline came back after a cut: read everything again (what came meanwhile was missed).
    resync: Arc<AtomicBool>,
    game_checked: Option<Instant>,
    machine: Arc<dyn Machine>,
    /// Where to go once the game the person starts by hand (a Mac) is up, and since when it waits.
    pending: Option<(Place, Instant)>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            loaded_for: None,
            loaded_at: None,
            failures: 0,
            retry_at: None,
            favourite_groups: Vec::new(),
            cooldowns: Cooldowns::default(),
            pictures: HashMap::new(),
            worn: BTreeMap::new(),
            worlds: HashMap::new(),
            lookup_failed: HashMap::new(),
            profiles: HashMap::new(),
            dismissed: None,
            pipeline: None,
            pipeline_client: None,
            accepted: Vec::new(),
            resync: Arc::new(AtomicBool::new(false)),
            game_checked: None,
            machine: real_machine(),
            pending: None,
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Everything VRChat says, read once: friends (online pages then offline pages), the account, favourite groups and who
/// is in them, invites and boops waiting. Each call is paced by the client (one a second per kind).
pub fn read_signed_in(client: &Mutex<Client>) -> Result<(LauncherState, Vec<(String, String)>, BTreeMap<String, Decorations>), Error> {
    let mut friends = BTreeMap::new();
    let mut worn = BTreeMap::new();
    for offline in [false, true] {
        for page in 0..50 {
            let items = client.lock().unwrap().friends_page(offline, page * 100)?;
            let n = items.len();
            for v in &items {
                if let Some(f) = launcher_feed::friend(v, !offline) {
                    worn.insert(f.id.clone(), launcher_feed::decorations(v));
                    friends.entry(f.id.clone()).or_insert(f);
                }
            }
            if n < 100 {
                break;
            }
        }
    }
    let me_value = client.lock().unwrap().current_user_value()?;
    let mut me_value = me_value;
    me_value["state"] = serde_json::Value::String("online".into());
    let me = launcher_feed::friend(&me_value, false);
    let groups = launcher_feed::favourite_groups(&client.lock().unwrap().favourite_groups_value()?);
    let members = launcher_feed::favourites(&client.lock().unwrap().favourite_friends_value()?);
    // Notifications are a nicety: their failure leaves the list.
    let invites: Vec<_> = client.lock().unwrap().notifications().unwrap_or_default().iter().filter_map(launcher_feed::invite).collect();
    let boops: Vec<_> = client.lock().unwrap().notifications_v2().unwrap_or_default().iter().filter_map(launcher_feed::boop).collect();
    let mut state = LauncherState {
        source: Source::SignedIn { user_id: me.as_ref().map(|m| m.id.clone()).unwrap_or_default(), name: me.as_ref().map(|m| m.name.clone()).unwrap_or_default() },
        possible: Possible { live_status: true, invites: true, request_invite: true, boop: true, play: true },
        me,
        friends: friends.into_values().collect(),
        invites,
        boops,
        boop_emojis: launcher_feed::boop_emojis(),
        ..LauncherState::default()
    };
    for f in &mut state.friends {
        f.favourite_groups = members.get(&f.id).cloned().unwrap_or_default();
    }
    let icons: HashMap<String, String> = state.friends.iter().map(|f| (f.id.clone(), f.look.icon.clone())).collect();
    for i in &mut state.invites {
        i.from_icon = icons.get(&i.from_id).cloned().unwrap_or_default();
    }
    for b in &mut state.boops {
        if b.from_icon.is_empty() {
            b.from_icon = icons.get(&b.from_id).cloned().unwrap_or_default();
        }
    }
    state.invites.sort_by_key(|i| std::cmp::Reverse(i.received_at));
    state.boops.sort_by_key(|b| std::cmp::Reverse(b.received_at));
    launcher_feed::regroup(&mut state, &groups);
    Ok((state, groups, worn))
}

/// How long after the `failures`-th failed read in a row the next one is: 30 s doubling to 10 minutes, spread by
/// `random` over three quarters to five quarters of that, and never before VRChat's own "try again in".
fn retry_wait(failures: u32, e: &Error, random: u16) -> Duration {
    let base = RETRY_FIRST.saturating_mul(1 << failures.saturating_sub(1).min(10)).min(RETRY_LONGEST);
    let wait = base.mul_f64(0.75 + f64::from(random) / 131072.0);
    match e {
        Error::RateLimited(d) => wait.max(*d),
        _ => wait,
    }
}

fn random() -> u16 {
    let b = *uuid::Uuid::new_v4().as_bytes();
    u16::from_be_bytes([b[0], b[1]])
}

/// A token for one pipeline connection: only a session that is over stops the pipeline; anything else (no network, a
/// time-out, a 429, a 5xx) is tried again after the pipeline's wait.
fn pipeline_token(client: &Mutex<Client>) -> Result<String, orbiters_core::pipeline::NoToken> {
    token_outcome(client.lock().unwrap().pipeline_token().map(|s| s.0))
}

fn token_outcome(token: Result<String, Error>) -> Result<String, orbiters_core::pipeline::NoToken> {
    use orbiters_core::pipeline::NoToken;
    token.map_err(|e| match e {
        Error::Unauthorized | Error::Disabled => NoToken::Refused,
        _ => NoToken::Later,
    })
}

/// The words for an action that could not be done (in the page's language).
pub fn error_text(e: &Error) -> String {
    e.to_string()
}

impl Controller {
    /// The signed-in VRChat client, when the feature and the session allow it.
    fn launcher_client(&self) -> Option<Arc<Mutex<Client>>> {
        let client = self.vrchat_signed_client()?;
        let signed = client.lock().unwrap().signed_in();
        signed.then_some(client)
    }

    /// The client, only when the list on show is this very account's: a press on a row read for another account (the
    /// sign-in changed, its new list is still being read) never sends from the new one.
    fn launcher_sender(&self) -> Result<Arc<Mutex<Client>>, Notice> {
        let Some(client) = self.launcher_client() else {
            return Err(Notice::Error(tr("Sign in to VRChat in Settings to boop and ask for invites.").into()));
        };
        let account = client.lock().unwrap().signed_in_user_id().map(str::to_owned);
        match (&self.view().launcher.source, account) {
            (Source::SignedIn { user_id, .. }, Some(account)) if *user_id == account => Ok(client),
            _ => Err(Notice::Error(tr("Your VRChat account changed: try again.").into())),
        }
    }

    pub(super) fn start_launcher(self: &Shared) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("launcher".into()).spawn(move || loop {
            let Some(ctl) = weak.upgrade() else { break };
            ctl.launcher_tick();
            drop(ctl);
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    /// One look: is the source still the one read, is a read due, is the game running.
    fn launcher_tick(self: &Shared) {
        let client = self.launcher_client();
        let account = client.as_ref().and_then(|c| c.lock().unwrap().signed_in_user_id().map(str::to_owned));
        let mut reopen = false;
        let (due, machine, check_game) = {
            let mut rt = self.launcher.lock().unwrap();
            let changed = rt.loaded_for.as_ref() != Some(&account);
            // The same account signed in again (a new grant, a new client): its old connection stops, a new one opens.
            let regranted = !changed && rt.pipeline.is_some() && !matches!((&rt.pipeline_client, &client), (Some(a), Some(b)) if Arc::ptr_eq(a, b));
            if changed || regranted {
                if let Some(stop) = rt.pipeline.take() {
                    stop.store(true, Ordering::Release);
                }
                rt.pipeline_client = None;
            }
            reopen = regranted && client.is_some();
            if changed {
                rt.pictures.clear();
                rt.worn.clear();
            }
            if changed {
                rt.failures = 0;
                rt.retry_at = None;
                // What one account was shown (or refused) is not another's.
                rt.worlds.clear();
                rt.lookup_failed.clear();
                rt.profiles.clear();
            }
            let every = if account.is_some() { FULL_EVERY } else { LOCAL_EVERY };
            // The pipeline came back: a full read, but not more often than every few minutes (kept pending meanwhile).
            let resync = rt.resync.load(Ordering::Acquire) && rt.loaded_at.is_none_or(|t| t.elapsed() > RESYNC_AFTER);
            let due = changed
                || match rt.retry_at {
                    // After a failure only its own clock counts.
                    Some(at) => Instant::now() >= at,
                    None => resync || rt.loaded_at.is_none_or(|t| t.elapsed() > every),
                };
            if due {
                rt.resync.store(false, Ordering::Release);
            }
            let check_game = rt.game_checked.is_none_or(|t| t.elapsed() > GAME_EVERY);
            if check_game {
                rt.game_checked = Some(Instant::now());
            }
            (due, rt.machine.clone(), check_game)
        };
        if check_game {
            self.launcher_game(machine.as_ref());
        }
        if let (true, Some(client)) = (reopen, &client) {
            self.launcher_pipeline(client.clone());
        }
        if due {
            if self.view().launcher.friends.is_empty() {
                self.update_if(|s| !std::mem::replace(&mut s.launcher.loading, true));
            }
            match &client {
                Some(client) => self.launcher_read_signed_in(client, account.clone()),
                None => self.launcher_read_local(),
            }
        }
        self.launcher_settle_accepted(client.as_ref());
        if let Some(client) = &client {
            self.launcher_resolve_pictures(client);
            self.launcher_resolve_worlds(client);
        }
    }

    /// The names and pictures of the worlds friends are in (VRChat's friend list names the instance only): two a look,
    /// each kept for the session (the client keeps worlds a day too).
    fn launcher_resolve_worlds(&self, client: &Arc<Mutex<Client>>) {
        let wanted: Vec<String> = {
            let rt = self.launcher.lock().unwrap();
            let mut ids = unnamed_worlds(&self.view().launcher);
            ids.retain(|id| !rt.worlds.contains_key(id) && rt.lookup_failed.get(id).is_none_or(|t| t.elapsed() > LOOKUP_AGAIN));
            ids.truncate(2);
            ids
        };
        let mut found = Vec::new();
        let mut failed = Vec::new();
        for id in wanted {
            match client.lock().unwrap().world(&id) {
                Ok(w) => found.push((id, Some((w.name, if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url })))),
                // Not shown to this account (private, deleted): asked no more.
                Err(Error::NotFound) => found.push((id, None)),
                Err(Error::RateLimited(_)) => break,
                // Passing (a time-out, VRChat unreachable): again later, not never.
                Err(_) => failed.push(id),
            }
        }
        let worlds = {
            let mut rt = self.launcher.lock().unwrap();
            rt.worlds.extend(found);
            for id in failed {
                rt.lookup_failed.insert(id, Instant::now());
            }
            // Nothing known to write in (every unnamed world still unknown or not given): no copy, no publish. A world
            // already known is still written into a friend who just moved there.
            if !unnamed_worlds(&self.view().launcher).iter().any(|id| matches!(rt.worlds.get(id), Some(Some(_)))) {
                return;
            }
            rt.worlds.clone()
        };
        self.update_if(|s| {
            let mut state = (*s.launcher).clone();
            fill_worlds(&mut state, &worlds);
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
    }

    fn launcher_game(self: &Shared, machine: &dyn Machine) {
        let view = self.view();
        // A Mac runs its own VRChat through Obelisk whatever it follows on the network; where Obelisk is not, a Mac
        // that follows a VRChat elsewhere is not told to get it.
        let remote = view.vrchat.remote;
        let here = if machine.by_hand() { !(remote && machine.engine_missing()) } else { orbiters_core::osc::VRCHAT_RUNS_HERE && !remote };
        // The watchers list every VRChat every 3 s already: the processes are looked at here only when they see one, or
        // while a start is under way (a Mac's game is looked for by its own scan).
        let look = here && (machine.by_hand() || !view.vrchat.runs.is_empty() || view.vrchat.running || matches!(view.launcher.game, Game::Starting(_) | Game::Running(_)));
        let running = if look { machine.running() } else { Vec::new() };
        let game = match running.first() {
            Some((_, desktop)) => Game::Running(if *desktop { LaunchMode::Desktop } else { LaunchMode::Vr }),
            None if !here => Game::NotRunning,
            None => match view.launcher.game {
                // A start just asked for is "starting" until VRChat shows up (or a minute passes).
                Game::Starting(mode) if self.launcher_started_recently() => Game::Starting(mode),
                _ if machine.engine_missing() => Game::NoEngine,
                _ if !machine.installed() => Game::NotInstalled,
                _ => Game::NotRunning,
            },
        };
        if matches!(game, Game::Running(_)) {
            self.launcher_go_pending();
        }
        // Published only when something changed (this runs every few seconds).
        self.update_if(|s| {
            let changed = s.launcher.game != game || s.launcher.possible.play != here;
            if s.launcher.game != game {
                s.launcher.game = game;
            }
            if s.launcher.possible.play != here {
                s.launcher.possible.play = here;
            }
            changed
        });
    }

    fn launcher_started_recently(&self) -> bool {
        // A game the person starts by hand in Obelisk takes its sign-in, an update check and Easy Anti-Cheat first.
        let patience = if self.launcher.lock().unwrap().machine.by_hand() { Duration::from_secs(180) } else { STARTING_FOR };
        self.launcher_started.lock().unwrap().is_some_and(|t| t.elapsed() < patience)
    }

    /// The game started by hand is up: the destination that waited for it goes as an invite to oneself, which VRChat
    /// shows once the person is in. One that waited more than ten minutes is forgotten (the game was not started).
    fn launcher_go_pending(self: &Shared) {
        let Some((place, since)) = self.launcher.lock().unwrap().pending.take() else { return };
        if since.elapsed() > PENDING_FOR {
            return;
        }
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("launcher-pending".into()).spawn(move || {
            let notice = ctl.launcher_invite_self(&place);
            ctl.update(|s| s.launcher.notice = Some(notice));
        });
    }

    fn launcher_invite_self(&self, place: &Place) -> Notice {
        // Only for the account whose list is on show (AN-2).
        match self.launcher_sender().ok() {
            Some(client) => match client.lock().unwrap().invite_self(&place.world_id, &place.instance_id) {
                Ok(()) => Notice::Sent { to: place.world_id.clone(), action: SentKind::Join },
                Err(e) => Notice::Error(error_text(&e)),
            },
            None => Notice::Error(tr("Sign in to VRChat in Settings so an invite to this instance can be sent to you.").into()),
        }
    }

    fn launcher_read_local(&self) {
        let Ok(mut state) = orbiters_core::launcher_local::read_now() else {
            // Caught while VRCX / VRCNext write their copy: the list on show stays; again in a few seconds.
            let mut rt = self.launcher.lock().unwrap();
            rt.loaded_for = Some(None);
            rt.retry_at = Some(Instant::now() + LOCAL_RETRY);
            return;
        };
        let groups: Vec<(String, String)> = state.groups.iter().filter(|g| g.kind == orbiters_core::launcher::GroupKind::Favourite).map(|g| (g.id.clone(), g.name.clone())).collect();
        {
            let mut rt = self.launcher.lock().unwrap();
            rt.loaded_for = Some(None);
            rt.loaded_at = Some(Instant::now());
            rt.retry_at = None;
            rt.favourite_groups = groups;
        }
        self.update_if(|s| {
            state.game = s.launcher.game.clone();
            state.possible.play = s.launcher.possible.play;
            state.profile = s.launcher.profile.clone().filter(|p| state.friends.iter().any(|f| f.id == p.friend.id));
            state.notice = s.launcher.notice.clone();
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
    }

    fn launcher_read_signed_in(self: &Shared, client: &Arc<Mutex<Client>>, account: Option<String>) {
        let read = read_signed_in(client);
        // The sign-in changed while the pages were read: this read belongs to no one shown; the next look starts again.
        let now_account = self.launcher_client().and_then(|c| c.lock().unwrap().signed_in_user_id().map(str::to_owned));
        if now_account != account {
            return;
        }
        match read {
            Ok((mut state, groups, worn)) => {
                let pipeline = {
                    let mut rt = self.launcher.lock().unwrap();
                    rt.loaded_for = Some(account);
                    rt.loaded_at = Some(Instant::now());
                    rt.failures = 0;
                    rt.retry_at = None;
                    rt.favourite_groups = groups;
                    rt.worn = worn;
                    rt.cooldowns.mark(&mut state.friends, now());
                    // Accepted here and on the way: VRChat still lists them until the account arrives.
                    state.invites.retain(|i| !rt.accepted.iter().any(|(a, _)| a.id == i.id));
                    let dismissed = rt.dismissed.get_or_insert_with(load_dismissed);
                    state.boops.retain(|b| !dismissed.contains_key(&b.id));
                    for f in &mut state.friends {
                        if let Some(d) = rt.worn.get(&f.id) {
                            apply_pictures(&rt.pictures, d, &mut f.look);
                        }
                    }
                    rt.pipeline.is_none()
                };
                self.update(|s| {
                    state.game = s.launcher.game.clone();
                    state.possible.play = s.launcher.possible.play;
                    // An open profile stays only when it is one of this account's friends (or the account itself).
                    state.profile = s.launcher.profile.take().filter(|p| state.friends.iter().any(|f| f.id == p.friend.id) || state.me.as_ref().is_some_and(|m| m.id == p.friend.id));
                    state.notice = s.launcher.notice.take();
                    *s.launcher = state;
                });
                if pipeline {
                    self.launcher_pipeline(client.clone());
                }
            }
            Err(e) => {
                {
                    // Tried for this account: the next try after a growing wait, never every two seconds.
                    let mut rt = self.launcher.lock().unwrap();
                    rt.loaded_for = Some(account);
                    rt.failures += 1;
                    rt.retry_at = Some(Instant::now() + retry_wait(rt.failures, &e, random()));
                }
                let text = error_text(&e);
                // Said once per run of failures (an expired session, VRChat unreachable), not at every try again.
                if self.launcher.lock().unwrap().failures == 1 {
                    self.notify(super::NoticeKind::Error, text.clone());
                }
                self.update(|s| {
                    s.launcher.loading = false;
                    s.launcher.notice = Some(Notice::Error(text));
                });
            }
        }
    }

    /// The one pipeline connection, until the account changes (or the session ends).
    fn launcher_pipeline(self: &Shared, client: Arc<Mutex<Client>>) {
        let stop = Arc::new(AtomicBool::new(false));
        let resync = {
            let mut rt = self.launcher.lock().unwrap();
            rt.pipeline = Some(stop.clone());
            rt.pipeline_client = Some(client.clone());
            rt.resync.clone()
        };
        // Tests never open a websocket to VRChat.
        if cfg!(test) {
            return;
        }
        let weak = Arc::downgrade(self);
        let mine = stop.clone();
        let spawned = std::thread::Builder::new().name("vrchat-pipeline".into()).spawn(move || {
            let mut first = true;
            let ended = orbiters_core::pipeline::run(
                &stop,
                || pipeline_token(&client),
                |message| {
                    // A message of an account (or a sign-in) that is no longer the one shown is not applied.
                    if let Some(ctl) = weak.upgrade() {
                        ctl.launcher_message_of(&stop, message);
                    }
                },
                |up| {
                    if up && !std::mem::take(&mut first) {
                        resync.store(true, Ordering::Release);
                    }
                },
            );
            if ended == orbiters_core::pipeline::Ended::Refused {
                if let Some(ctl) = weak.upgrade() {
                    let mut rt = ctl.launcher.lock().unwrap();
                    if rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, &stop)) {
                        rt.pipeline = None;
                        rt.pipeline_client = None;
                    }
                }
            }
        });
        if spawned.is_err() {
            let mut rt = self.launcher.lock().unwrap();
            if rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, &mine)) {
                rt.pipeline = None;
                rt.pipeline_client = None;
            }
        }
    }

    /// One message of the pipeline `from`: applied only while it is still the launcher's pipeline (an old connection
    /// of an account, or of a sign-in, that was replaced never writes into the list on show).
    fn launcher_message_of(&self, from: &Arc<AtomicBool>, message: &str) {
        let current = self.launcher.lock().unwrap().pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, from)) && !from.load(Ordering::Acquire);
        if current {
            self.launcher_message(message);
        }
    }

    /// One pipeline message applied to the list.
    pub(super) fn launcher_message(&self, message: &str) {
        let (groups, dismissed) = {
            let mut rt = self.launcher.lock().unwrap();
            let dismissed: Vec<String> = rt.dismissed.get_or_insert_with(load_dismissed).keys().cloned().collect();
            (rt.favourite_groups.clone(), dismissed)
        };
        let mut applied = None;
        self.update_if(|s| {
            let mut state = (*s.launcher).clone();
            let a = launcher_feed::apply(&mut state, &groups, message, now());
            state.boops.retain(|b| !dismissed.contains(&b.id));
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            applied = Some(a);
            changed
        });
        let applied = applied.unwrap_or_default();
        if !applied.decorations.is_empty() {
            let mut rt = self.launcher.lock().unwrap();
            for (id, d) in applied.decorations {
                rt.worn.insert(id, d);
            }
        }
    }

    /// Decorations' pictures, a few per look (each one a call, paced), then written into the friends that wear them.
    fn launcher_resolve_pictures(&self, client: &Arc<Mutex<Client>>) {
        let wanted: Vec<String> = {
            let rt = self.launcher.lock().unwrap();
            let mut ids: Vec<String> = rt
                .worn
                .values()
                .flat_map(|d| [d.icon_frame.clone(), d.nameplate.clone()])
                .flatten()
                .filter(|id| !rt.pictures.contains_key(id) && rt.lookup_failed.get(id).is_none_or(|t| t.elapsed() > LOOKUP_AGAIN))
                .collect();
            ids.sort();
            ids.dedup();
            ids.truncate(3);
            ids
        };
        if wanted.is_empty() {
            return;
        }
        let mut found = Vec::new();
        let mut failed = Vec::new();
        for id in wanted {
            match client.lock().unwrap().inventory_template_picture(&id) {
                Ok(p) => found.push((id, Some(p))),
                Err(Error::NotFound | Error::InvalidResponse) => found.push((id, None)),
                Err(Error::RateLimited(_)) => break,
                Err(_) => failed.push(id),
            }
        }
        let (pictures, worn) = {
            let mut rt = self.launcher.lock().unwrap();
            rt.pictures.extend(found);
            for id in failed {
                rt.lookup_failed.insert(id, Instant::now());
            }
            (rt.pictures.clone(), rt.worn.clone())
        };
        self.update_if(|s| {
            let mut state = (*s.launcher).clone();
            for f in &mut state.friends {
                if let Some(d) = worn.get(&f.id) {
                    apply_pictures(&pictures, d, &mut f.look);
                }
            }
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
    }

    /// What Home sends. Runs on its own thread; the outcome shows in `launcher.notice` for a few seconds, and an action
    /// that could not be done is said in the app's toast too.
    pub fn launcher_action(self: &Shared, action: Action) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("launcher-action".into()).spawn(move || {
            let Some(notice) = ctl.launcher_do(action) else { return };
            ctl.launcher_notice(notice.clone());
            std::thread::sleep(NOTICE_FOR);
            ctl.launcher_notice_over(&notice);
        });
    }

    /// An action's outcome: into the state, and when it failed, into the toast as well.
    pub(super) fn launcher_notice(&self, notice: Notice) {
        if let Notice::Error(text) | Notice::Failed { text, .. } = &notice {
            self.notify(super::NoticeKind::Error, text.clone());
        }
        self.update(|s| s.launcher.notice = Some(notice));
    }

    /// The outcome's time is over: gone from the state, unless another action's replaced it.
    pub(super) fn launcher_notice_over(&self, notice: &Notice) {
        self.update_if(|s| {
            let same = s.launcher.notice.as_ref() == Some(notice);
            if same {
                s.launcher.notice = None;
            }
            same
        });
    }

    pub(super) fn launcher_do(self: &Shared, action: Action) -> Option<Notice> {
        match action {
            Action::Play { mode, destination } => Some(self.launcher_play(mode, destination)),
            Action::AcceptInvite { invite, mode } => {
                // The invite is the account's on show: never accepted with another account's session.
                if self.launcher_client().is_some() {
                    if let Err(notice) = self.launcher_sender() {
                        return Some(notice);
                    }
                }
                let found = self.view().launcher.invites.iter().find(|i| i.id == invite).cloned();
                let Some(place) = found.and_then(|i| i.place) else { return Some(Notice::Error(tr("This invite is no longer here.").into())) };
                let notice = self.launcher_play(mode, Some(place));
                if !matches!(notice, Notice::Error(_)) {
                    // The card goes here at once; VRChat hides the invite only once the account got there (a start that
                    // fails, or lands at home, leaves it in VRChat and brings the card back).
                    if let Some(found) = self.view().launcher.invites.iter().find(|i| i.id == invite).cloned() {
                        self.launcher.lock().unwrap().accepted.push((found, Instant::now()));
                    }
                    self.update(|s| s.launcher.invites.retain(|i| i.id != invite));
                }
                Some(notice)
            }
            Action::Dismiss { id } => {
                self.launcher_dismiss(&id);
                None
            }
            Action::RequestInvite { to } => Some(self.launcher_send(false, &to, None)),
            Action::Boop { to, emoji } => Some(self.launcher_send(true, &to, emoji.as_deref())),
            Action::OpenProfile { id } => {
                self.launcher_profile(&id);
                None
            }
            Action::OpenEngine => {
                let machine = self.launcher.lock().unwrap().machine.clone();
                machine.open_engine().err().map(Notice::Error)
            }
            Action::CloseProfile => {
                self.update(|s| s.launcher.profile = None);
                None
            }
            Action::Refresh => {
                let mut rt = self.launcher.lock().unwrap();
                rt.loaded_at = None;
                rt.retry_at = None;
                None
            }
        }
    }

    fn launcher_play(self: &Shared, mode: LaunchMode, destination: Option<Place>) -> Notice {
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        // Ids come from the network (a friend's location, an invite) or over IPC: only VRChat's own grammar goes on.
        if destination.as_ref().is_some_and(|p| !orbiters_core::vrchat_launch::is_destination(&p.world_id, &p.instance_id)) {
            return Notice::Error(tr("VRChat does not know this world.").into());
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        // One start at a time: held from the look at the running VRChats to the start, so that two presses (or a press
        // and an invite's Accept, on their own threads) never start two; and none while one is starting.
        let mut started = self.launcher_started.lock().unwrap();
        let running = machine.running();
        let settings = self.view().settings.launch.clone();
        if !running.is_empty() {
            drop(started);
            // Never a second VRChat: the running one goes there.
            let Some(place) = destination else { return Notice::Error(tr("VRChat is already running.").into()) };
            if machine.by_hand() {
                return self.launcher_invite_self(&place);
            }
            let travel: Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| Arc::new(super::travel::Real));
            let bridge = super::travel::Bridge { main: self.sides.main_pid(), sides: self.sides.side_pids() };
            return match super::travel::go(travel.as_ref(), false, &bridge, &place.world_id, &place.instance_id) {
                Ok(_) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                Err(super::travel::GoFailure::Invite(_)) => match self.launcher_sender().ok() {
                    Some(client) => match client.lock().unwrap().invite_self(&place.world_id, &place.instance_id) {
                        Ok(()) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                        Err(e) => Notice::Error(error_text(&e)),
                    },
                    None => Notice::Error(tr("VRChat did not take the link. Sign in to VRChat in Settings so an invite can be sent instead.").into()),
                },
                Err(super::travel::GoFailure::BridgeBusy) => Notice::Error(tr("Only the assistant's VRChat is running. Close it first.").into()),
                Err(super::travel::GoFailure::Start(words)) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
                Err(super::travel::GoFailure::BadDestination) => Notice::Error(tr("VRChat does not know this world.").into()),
            };
        }
        if machine.by_hand() {
            return self.launcher_play_by_hand(machine.as_ref(), destination, &mut started);
        }
        if started.is_some_and(|t| t.elapsed() < STARTING_FOR) {
            return Notice::Error(tr("Starting VRChat…").into());
        }
        if !machine.installed() {
            return Notice::Error(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into());
        }
        let mut args = orbiters_core::vrchat_launch::arguments(mode, &settings, destination.as_ref().map(|p| (p.world_id.as_str(), p.instance_id.as_str())));
        // The window VRChat saved (read while none runs): kept before a VR start shrinks it, given back to Desktop.
        let saved = machine.saved_window();
        let mut keep_window = None;
        match mode {
            LaunchMode::Desktop => {
                let extra = desktop_window(saved, settings.desktop_window);
                let link = args.iter().position(|a| a.starts_with("vrchat://")).unwrap_or(args.len());
                args.splice(link..link, extra);
            }
            LaunchMode::Vr if settings.performance && settings.small_window_in_vr => keep_window = saved.filter(SavedWindow::normal).filter(|w| settings.desktop_window != Some(*w)),
            LaunchMode::Vr => {}
        }
        match machine.start(&args) {
            Ok(()) => {
                *started = Some(Instant::now());
                drop(started);
                self.update(|s| s.launcher.game = Game::Starting(mode));
                if settings.last_mode != mode || keep_window.is_some() {
                    self.update_settings(|s| {
                        s.launch.last_mode = mode;
                        if keep_window.is_some() {
                            s.launch.desktop_window = keep_window;
                        }
                    });
                }
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Accepted invites: hidden at VRChat once the account is in their instance; after [`ARRIVE_WITHIN`] without arriving,
    /// their card comes back.
    fn launcher_settle_accepted(&self, client: Option<&Arc<Mutex<Client>>>) {
        let here = match &self.view().launcher.me {
            Some(me) => match &me.whereabouts {
                Whereabouts::World(p) => Some((p.world_id.clone(), p.instance_id.clone())),
                _ => None,
            },
            None => None,
        };
        let (arrived, back) = {
            let mut rt = self.launcher.lock().unwrap();
            if rt.accepted.is_empty() {
                return;
            }
            let (arrived, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut rt.accepted)
                .into_iter()
                .partition(|(i, _)| i.place.as_ref().is_some_and(|p| here.as_ref().is_some_and(|(w, inst)| *w == p.world_id && *inst == p.instance_id)));
            let (back, waiting): (Vec<_>, Vec<_>) = waiting.into_iter().partition(|(_, at)| at.elapsed() > ARRIVE_WITHIN);
            rt.accepted = waiting;
            (arrived, back)
        };
        if let Some(client) = client {
            for (invite, _) in &arrived {
                let _ = client.lock().unwrap().hide_notification(&invite.id);
            }
        }
        if !back.is_empty() {
            self.update(|s| {
                for (invite, _) in back {
                    if !s.launcher.invites.iter().any(|i| i.id == invite.id) {
                        s.launcher.invites.push(invite);
                    }
                }
                s.launcher.invites.sort_by_key(|i| std::cmp::Reverse(i.received_at));
            });
        }
    }

    /// A Mac: Obelisk is opened for the person to press Play in; the destination waits for the game.
    /// `started` is the start's lock, held by [`Self::launcher_play`].
    fn launcher_play_by_hand(self: &Shared, machine: &dyn Machine, destination: Option<Place>, started: &mut Option<Instant>) -> Notice {
        if machine.engine_missing() {
            let _ = machine.open_engine();
            return Notice::Error(tr("VRChat runs on a Mac through Obelisk, a free app. Get it, turn on Beta versions in its settings, then download VRChat in it.").into());
        }
        if !machine.installed() {
            let _ = machine.open_engine();
            return Notice::Error(tr("Download VRChat in Obelisk (with Beta versions on in its settings), then come back.").into());
        }
        match machine.start(&[]) {
            Ok(()) => {
                *started = Some(Instant::now());
                self.launcher.lock().unwrap().pending = destination.clone().map(|p| (p, Instant::now()));
                self.update(|s| s.launcher.game = Game::Starting(LaunchMode::Desktop));
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Hides an invite, a request or a boop here; an invite or a request is hidden at VRChat too (nobody is told).
    fn launcher_dismiss(&self, id: &str) {
        let was_invite = self.view().launcher.invites.iter().any(|i| i.id == id);
        if self.view().launcher.boops.iter().any(|b| b.id == id) {
            // VRChat lists a boop again at each full read: remembered here, so that it does not come back.
            let mut rt = self.launcher.lock().unwrap();
            let at = now();
            let dismissed = rt.dismissed.get_or_insert_with(load_dismissed);
            dismissed.retain(|_, t| at - *t < DISMISSED_FOR);
            dismissed.insert(id.to_string(), at);
            save_dismissed(dismissed);
        }
        self.update(|s| {
            s.launcher.invites.retain(|i| i.id != id);
            s.launcher.boops.retain(|b| b.id != id);
        });
        if was_invite {
            if let Ok(client) = self.launcher_sender() {
                let _ = client.lock().unwrap().hide_notification(id);
            }
        }
    }

    fn launcher_send(&self, boop: bool, to: &str, emoji: Option<&str>) -> Notice {
        let client = match self.launcher_sender() {
            Ok(client) => client,
            Err(notice) => return notice,
        };
        let at = now();
        let taken = self.launcher.lock().unwrap().cooldowns.take(boop, to, at);
        if let Err(until) = taken {
            return Notice::CoolingDown { to: to.into(), until };
        }
        let sent = if boop { client.lock().unwrap().boop(to, emoji) } else { client.lock().unwrap().request_invite(to) };
        let notice = match sent {
            Ok(()) => Notice::Sent { to: to.into(), action: if boop { SentKind::Boop } else { SentKind::RequestInvite } },
            Err(e) => {
                // Not sent (refused or never left): the person may try again at once. A maybe-sent keeps its cool-down.
                if !matches!(e, Error::OutcomeUnknown) {
                    self.launcher.lock().unwrap().cooldowns.release(boop, to);
                }
                Notice::Failed { to: to.into(), action: if boop { SentKind::Boop } else { SentKind::RequestInvite }, text: error_text(&e) }
            }
        };
        let cooldowns = self.launcher.lock().unwrap().cooldowns.clone();
        self.update(|s| cooldowns.mark(&mut s.launcher.friends, now()));
        notice
    }

    fn launcher_profile(&self, id: &str) {
        let Some(friend) = self.view().launcher.friend(id).cloned().or_else(|| self.view().launcher.me.clone().filter(|m| m.id == id)) else { return };
        let client = self.launcher_client();
        self.update(|s| s.launcher.profile = Some(Profile { friend: friend.clone(), loading: client.is_some(), ..Profile::default() }));
        let Some(client) = client else { return };
        let kept = self.launcher.lock().unwrap().profiles.get(id).filter(|(at, ..)| at.elapsed() < PROFILE_FOR).map(|(_, p, u, m)| (p.clone(), u.clone(), *m));
        let (public, user, mutuals) = match kept {
            Some(kept) => kept,
            None => {
                // Each call only while this profile is still the one open (clicking down a list asks for the last one).
                let wanted = || self.view().launcher.profile.as_ref().is_some_and(|p| p.friend.id == id);
                if !wanted() {
                    return;
                }
                let public = client.lock().unwrap().public_profile(id).unwrap_or(serde_json::Value::Null);
                if !wanted() {
                    return;
                }
                let user = client.lock().unwrap().user_value(id).unwrap_or(serde_json::Value::Null);
                if !wanted() {
                    return;
                }
                let mutuals = client.lock().unwrap().mutuals(id).ok();
                let mut rt = self.launcher.lock().unwrap();
                if rt.profiles.len() >= PROFILES_KEPT {
                    rt.profiles.retain(|_, (at, ..)| at.elapsed() < PROFILE_FOR);
                    if rt.profiles.len() >= PROFILES_KEPT {
                        rt.profiles.clear();
                    }
                }
                rt.profiles.insert(id.to_string(), (Instant::now(), public.clone(), user.clone(), mutuals));
                (public, user, mutuals)
            }
        };
        let profile = launcher_feed::profile(friend, &public, &user, mutuals);
        self.update(|s| {
            if s.launcher.profile.as_ref().is_some_and(|p| p.friend.id == profile.friend.id) {
                s.launcher.profile = Some(profile);
            }
        });
    }

    #[cfg(test)]
    pub(super) fn launcher_machine(&self, machine: Arc<dyn Machine>) {
        self.launcher.lock().unwrap().machine = machine;
    }
}

fn dismissed_file() -> Option<std::path::PathBuf> {
    // Tests keep it in memory: never the person's own file.
    if cfg!(test) {
        return None;
    }
    orbiters_core::paths::roots().map(|r| r.config.join("launcher-dismissed.json"))
}

fn load_dismissed() -> BTreeMap<String, i64> {
    let at = now();
    let mut kept: BTreeMap<String, i64> = dismissed_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    kept.retain(|_, t| at - *t < DISMISSED_FOR);
    kept
}

fn save_dismissed(kept: &BTreeMap<String, i64>) {
    if let (Some(file), Ok(bytes)) = (dismissed_file(), serde_json::to_vec(kept)) {
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
}

/// The worlds named by a place whose name is not known yet, friends in the list's order first.
pub fn unnamed_worlds(state: &LauncherState) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let by_id: HashMap<&str, &orbiters_core::launcher::Friend> = state.friends.iter().map(|f| (f.id.as_str(), f)).collect();
    let places = state
        .invites
        .iter()
        .filter_map(|i| i.place.as_ref())
        .chain(state.groups.iter().flat_map(|g| g.members.iter()).filter_map(|id| by_id.get(id.as_str()).copied()).filter_map(|f| match &f.whereabouts {
            orbiters_core::launcher::Whereabouts::World(p) => Some(p),
            _ => None,
        }));
    for p in places {
        if p.world_name.is_empty() && !ids.contains(&p.world_id) {
            ids.push(p.world_id.clone());
        }
    }
    ids
}

/// Writes the known worlds' names and pictures into every place of the state.
pub fn fill_worlds(state: &mut LauncherState, worlds: &HashMap<String, Option<(String, String)>>) {
    let fill = |p: &mut Place| {
        if let Some(Some((name, picture))) = worlds.get(&p.world_id) {
            if p.world_name.is_empty() {
                p.world_name = name.clone();
            }
            if p.world_picture.is_empty() {
                p.world_picture = picture.clone();
            }
        }
    };
    let friends = state.friends.iter_mut().chain(state.me.iter_mut()).chain(state.profile.iter_mut().map(|p| &mut p.friend));
    for f in friends {
        if let orbiters_core::launcher::Whereabouts::World(p) = &mut f.whereabouts {
            fill(p);
        }
    }
    for i in &mut state.invites {
        if let Some(p) = &mut i.place {
            fill(p);
        }
    }
}

fn apply_pictures(pictures: &HashMap<String, Option<String>>, d: &Decorations, look: &mut orbiters_core::launcher::Look) {
    let get = |id: &Option<String>| id.as_ref().and_then(|id| pictures.get(id).cloned().flatten()).unwrap_or_default();
    look.icon_frame = get(&d.icon_frame);
    look.nameplate = get(&d.nameplate);
}

#[cfg(test)]
#[path = "launcher_tests.rs"]
mod tests;
