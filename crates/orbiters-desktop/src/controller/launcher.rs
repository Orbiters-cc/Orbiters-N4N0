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
const FULL_EVERY: Duration = Duration::from_secs(30 * 60);
const GAME_EVERY: Duration = Duration::from_secs(5);
/// A start just asked for counts as "starting" this long (Steam, then Easy Anti-Cheat, then VRChat's process).
const STARTING_FOR: Duration = Duration::from_secs(60);
/// After a failed read of VRChat: the first try again, doubling up to the longest (with jitter), never every tick.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_LONGEST: Duration = Duration::from_secs(10 * 60);
/// A pipeline that came back asks for a full read only when the last one is older than this (else it waits until then).
const RESYNC_AFTER: Duration = Duration::from_secs(5 * 60);
/// An accepted invite is hidden at VRChat once the account is in its instance; if it never gets there in this time, the
/// card comes back (and VRChat still has the invite).
const ARRIVE_WITHIN: Duration = Duration::from_secs(10 * 60);

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
        width: dword("Screenmanager Resolution Width_h182942802")?,
        height: dword("Screenmanager Resolution Height_h2627697771")?,
        mode: dword("Screenmanager Fullscreen mode_h3630240806").unwrap_or(3),
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
    pipeline: Option<Arc<AtomicBool>>,
    /// Invites accepted here, waiting for the account to arrive in their instance before VRChat is told to hide them.
    accepted: Vec<(Invite, Instant)>,
    /// The pipeline came back after a cut: read everything again (what came meanwhile was missed).
    resync: Arc<AtomicBool>,
    game_checked: Option<Instant>,
    machine: Arc<dyn Machine>,
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
            pipeline: None,
            accepted: Vec::new(),
            resync: Arc::new(AtomicBool::new(false)),
            game_checked: None,
            machine: Arc::new(RealMachine::default()),
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
        let (due, machine, check_game) = {
            let mut rt = self.launcher.lock().unwrap();
            let changed = rt.loaded_for.as_ref() != Some(&account);
            if changed {
                if let Some(stop) = rt.pipeline.take() {
                    stop.store(true, Ordering::Release);
                }
                rt.pictures.clear();
                rt.worn.clear();
            }
            if changed {
                rt.failures = 0;
                rt.retry_at = None;
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
        if due {
            if self.view().launcher.friends.is_empty() {
                self.update(|s| s.launcher.loading = true);
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
            ids.retain(|id| !rt.worlds.contains_key(id));
            ids.truncate(2);
            ids
        };
        let mut found = Vec::new();
        for id in wanted {
            let world = client.lock().unwrap().world(&id);
            if matches!(world, Err(Error::RateLimited(_))) {
                break;
            }
            found.push((id, world.ok().map(|w| (w.name, if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url }))));
        }
        let worlds = {
            let mut rt = self.launcher.lock().unwrap();
            rt.worlds.extend(found);
            rt.worlds.clone()
        };
        self.update(|s| {
            let mut state = (*s.launcher).clone();
            fill_worlds(&mut state, &worlds);
            if state != *s.launcher {
                *s.launcher = state;
            }
        });
    }

    fn launcher_game(&self, machine: &dyn Machine) {
        let view = self.view();
        let here = orbiters_core::osc::VRCHAT_RUNS_HERE && !view.vrchat.remote;
        // The watchers list every VRChat every 3 s already: the processes are looked at here only when they see one, or
        // while a start is under way.
        let look = here && (!view.vrchat.runs.is_empty() || view.vrchat.running || matches!(view.launcher.game, Game::Starting(_) | Game::Running(_)));
        let running = if look { machine.running() } else { Vec::new() };
        let game = match running.first() {
            Some((_, desktop)) => Game::Running(if *desktop { LaunchMode::Desktop } else { LaunchMode::Vr }),
            None if !here => Game::NotRunning,
            None => match view.launcher.game {
                // A start just asked for is "starting" until VRChat shows up (or a minute passes).
                Game::Starting(mode) if self.launcher_started_recently() => Game::Starting(mode),
                _ if !machine.installed() => Game::NotInstalled,
                _ => Game::NotRunning,
            },
        };
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
        self.launcher_started.lock().unwrap().is_some_and(|t| t.elapsed() < STARTING_FOR)
    }

    fn launcher_read_local(&self) {
        let mut state = orbiters_core::launcher_local::read();
        let groups: Vec<(String, String)> = state.groups.iter().filter(|g| g.kind == orbiters_core::launcher::GroupKind::Favourite).map(|g| (g.id.clone(), g.name.clone())).collect();
        {
            let mut rt = self.launcher.lock().unwrap();
            rt.loaded_for = Some(None);
            rt.loaded_at = Some(Instant::now());
            rt.favourite_groups = groups;
        }
        self.update(|s| {
            state.game = s.launcher.game.clone();
            state.possible.play = s.launcher.possible.play;
            state.profile = s.launcher.profile.take().filter(|p| state.friends.iter().any(|f| f.id == p.friend.id));
            state.notice = s.launcher.notice.take();
            *s.launcher = state;
        });
    }

    fn launcher_read_signed_in(self: &Shared, client: &Arc<Mutex<Client>>, account: Option<String>) {
        match read_signed_in(client) {
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
                    state.profile = s.launcher.profile.take();
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
            rt.resync.clone()
        };
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("vrchat-pipeline".into()).spawn(move || {
            let mut first = true;
            let ended = orbiters_core::pipeline::run(
                &stop,
                || pipeline_token(&client),
                |message| {
                    // A message of an account that is no longer the one shown is not applied.
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(ctl) = weak.upgrade() {
                        ctl.launcher_message(message);
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
                    }
                }
            }
        });
    }

    /// One pipeline message applied to the list.
    pub(super) fn launcher_message(&self, message: &str) {
        let groups = self.launcher.lock().unwrap().favourite_groups.clone();
        let applied = self.update(|s| {
            let mut state = (*s.launcher).clone();
            let applied = launcher_feed::apply(&mut state, &groups, message, now());
            if applied.changed || applied.arrived || applied.me || state != *s.launcher {
                *s.launcher = state;
            }
            applied
        });
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
            let mut ids: Vec<String> = rt.worn.values().flat_map(|d| [d.icon_frame.clone(), d.nameplate.clone()]).flatten().filter(|id| !rt.pictures.contains_key(id)).collect();
            ids.sort();
            ids.dedup();
            ids.truncate(3);
            ids
        };
        if wanted.is_empty() {
            return;
        }
        let mut found = Vec::new();
        for id in wanted {
            let picture = client.lock().unwrap().inventory_template_picture(&id);
            if matches!(picture, Err(Error::RateLimited(_))) {
                break;
            }
            found.push((id, picture.ok()));
        }
        let (pictures, worn) = {
            let mut rt = self.launcher.lock().unwrap();
            rt.pictures.extend(found);
            (rt.pictures.clone(), rt.worn.clone())
        };
        self.update(|s| {
            let mut state = (*s.launcher).clone();
            for f in &mut state.friends {
                if let Some(d) = worn.get(&f.id) {
                    apply_pictures(&pictures, d, &mut f.look);
                }
            }
            if state != *s.launcher {
                *s.launcher = state;
            }
        });
    }

    /// What Home sends. Runs on its own thread; the outcome shows in `launcher.notice`.
    pub fn launcher_action(self: &Shared, action: Action) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("launcher-action".into()).spawn(move || {
            let notice = ctl.launcher_do(action);
            if let Some(notice) = notice {
                ctl.update(|s| s.launcher.notice = Some(notice));
            }
        });
    }

    pub(super) fn launcher_do(self: &Shared, action: Action) -> Option<Notice> {
        match action {
            Action::Play { mode, destination } => Some(self.launcher_play(mode, destination)),
            Action::AcceptInvite { invite, mode } => {
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
            let travel: Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| Arc::new(super::travel::Real));
            let bridge = super::travel::Bridge { main: self.sides.main_pid(), sides: self.sides.side_pids() };
            return match super::travel::go(travel.as_ref(), false, &bridge, &place.world_id, &place.instance_id) {
                Ok(_) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                Err(super::travel::GoFailure::Invite(_)) => match self.launcher_client() {
                    Some(client) => match client.lock().unwrap().invite_self(&place.world_id, &place.instance_id) {
                        Ok(()) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                        Err(e) => Notice::Error(error_text(&e)),
                    },
                    None => Notice::Error(tr("VRChat did not take the link. Sign in to VRChat in Settings so an invite can be sent instead.").into()),
                },
                Err(super::travel::GoFailure::BridgeBusy) => Notice::Error(tr("Only the assistant's VRChat is running. Close it first.").into()),
                Err(super::travel::GoFailure::Start(words)) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
            };
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

    /// Hides an invite, a request or a boop here; an invite or a request is hidden at VRChat too (nobody is told).
    fn launcher_dismiss(&self, id: &str) {
        let was_invite = self.view().launcher.invites.iter().any(|i| i.id == id);
        self.update(|s| {
            s.launcher.invites.retain(|i| i.id != id);
            s.launcher.boops.retain(|b| b.id != id);
        });
        if was_invite {
            if let Some(client) = self.launcher_client() {
                let _ = client.lock().unwrap().hide_notification(id);
            }
        }
    }

    fn launcher_send(&self, boop: bool, to: &str, emoji: Option<&str>) -> Notice {
        let Some(client) = self.launcher_client() else {
            return Notice::Error(tr("Sign in to VRChat in Settings to boop and ask for invites.").into());
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
                Notice::Error(error_text(&e))
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
        let public = client.lock().unwrap().public_profile(id).unwrap_or(serde_json::Value::Null);
        let user = client.lock().unwrap().user_value(id).unwrap_or(serde_json::Value::Null);
        let mutuals = client.lock().unwrap().mutuals(id).ok();
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

/// The worlds named by a place whose name is not known yet, friends in the list's order first.
pub fn unnamed_worlds(state: &LauncherState) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let places = state
        .invites
        .iter()
        .filter_map(|i| i.place.as_ref())
        .chain(state.groups.iter().flat_map(|g| g.members.iter()).filter_map(|id| state.friend(id)).filter_map(|f| match &f.whereabouts {
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
