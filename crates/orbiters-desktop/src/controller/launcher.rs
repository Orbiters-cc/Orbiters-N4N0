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
/// The same on a Mac, where the person presses Play in Obelisk (its sign-in, an update check, Easy Anti-Cheat).
const MAC_STARTING_FOR: Duration = Duration::from_secs(180);
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
    /// Starts one more VRChat beside those running, with these arguments (`--profile=N` among them): through VRChat's own
    /// launcher, as the bridge does, since Steam starts a game once.
    fn start_beside(&self, args: &[String]) -> Result<(), String> {
        self.start(args)
    }
    /// The VRChats running here with their profile: process id, Desktop mode, `--profile` (0 without one).
    fn running_profiles(&self) -> Vec<(u32, bool, u32)> {
        self.running().into_iter().map(|(pid, desktop)| (pid, desktop, 0)).collect()
    }
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
    /// Each VRChat's Desktop mode and profile, read once from its command line.
    desktop: Mutex<HashMap<u32, (bool, u32)>>,
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

/// The VRChats running (process, Desktop mode, profile): the profile as their own log said it when the roster knows the
/// process, else as their command line does.
fn runs_with_profiles(machine: &dyn Machine, roster: &[crate::state::VrchatRunShown]) -> Vec<(u32, bool, u32)> {
    machine.running_profiles().into_iter().map(|(pid, desktop, profile)| (pid, desktop, roster.iter().find(|r| r.pid == pid).map_or(profile, |r| r.profile))).collect()
}

/// Home's profile choice: every profile used before (with the account last seen on it), running or starting now, by
/// number; profile 0 always.
pub fn game_profiles(
    known: &BTreeMap<u32, String>,
    runs: &[(u32, bool, u32)],
    roster: &[crate::state::VrchatRunShown],
    starting: Option<(u32, LaunchMode)>,
) -> Vec<orbiters_core::launcher::GameProfile> {
    let mut numbers: Vec<u32> = std::iter::once(0).chain(known.keys().copied()).chain(runs.iter().map(|r| r.2)).chain(starting.map(|s| s.0)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
        .into_iter()
        .map(|profile| {
            let run = runs.iter().find(|r| r.2 == profile);
            let shown = run.and_then(|r| roster.iter().find(|s| s.pid == r.0));
            orbiters_core::launcher::GameProfile {
                profile,
                account: shown.and_then(|s| s.account.clone()).or_else(|| known.get(&profile).cloned()),
                running: run.map(|r| if r.1 { LaunchMode::Desktop } else { LaunchMode::Vr }),
                starting: starting.filter(|s| s.0 == profile && run.is_none()).map(|s| s.1),
                assistant: shown.is_some_and(|s| s.driver != crate::state::VrchatDriverShown::Person),
            }
        })
        .collect()
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
        self.running_profiles().into_iter().map(|(pid, desktop, _)| (pid, desktop)).collect()
    }

    fn running_profiles(&self) -> Vec<(u32, bool, u32)> {
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
                let args: Vec<String> = system.process(pid).map(|p| p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect()).unwrap_or_default();
                let no_vr = args.iter().any(|a| a == "--no-vr");
                desktop.insert(pid.as_u32(), (no_vr, orbiters_core::vrchat_launch::profile_of(&args)));
            }
        }
        pids.iter().map(|p| {
            let (no_vr, profile) = desktop.get(&p.as_u32()).copied().unwrap_or((false, 0));
            (p.as_u32(), no_vr, profile)
        }).collect()
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

    fn start_beside(&self, args: &[String]) -> Result<(), String> {
        // VRChat's own launch.exe with `--profile`: a process of its own every time (measured 2026-11-15, bridge).
        crate::bridge::win::start_vrchat_beside(args).map_err(|f| f.message)
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
    accepted: Vec<Accepted>,
    /// Accepted invites whose hide is being sent now.
    hiding: Vec<String>,
    /// Bumped at each profile opened: an older request's answer writes nothing.
    profile_request: u64,
    /// Bumped at every change of the source on show (another account, a new sign-in, signed out): work started for an
    /// older one (a profile read) writes nothing.
    epoch: u64,
    /// The pipeline came back after a cut: read everything again (what came meanwhile was missed).
    resync: Arc<AtomicBool>,
    game_checked: Option<Instant>,
    machine: Arc<dyn Machine>,
    /// Where to go once the game the person starts by hand (a Mac) is up, since when it waits, and the account it was
    /// asked for (it goes to that account only).
    pending: Option<(Place, Instant, Option<String>)>,
    /// The profile a start from Home's profile choice asked for, its mode and when: "starting" until seen running.
    starting: Option<(u32, LaunchMode, Instant)>,
}

/// An invite accepted here, until VRChat is told to hide it (the account got there) or its card comes back.
#[derive(Clone)]
struct Accepted {
    invite: Invite,
    at: Instant,
    /// The account whose invite it is: only its own session hides it, only its own list gets the card back.
    owner: String,
    /// Where the account was at the press: being there already is no proof of having arrived.
    from: Option<(String, String)>,
    /// Arrived; the hide failed this many times (it is tried again a minute later, at most [`HIDE_TRIES`] times).
    hide_failures: u32,
    hide_again: Option<Instant>,
    /// The account was seen somewhere else than `from` since the press: coming back to `from` is an arrival then.
    moved: bool,
}

const HIDE_TRIES: u32 = 5;
const HIDE_AGAIN: Duration = Duration::from_secs(60);

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
            hiding: Vec::new(),
            profile_request: 0,
            epoch: 0,
            resync: Arc::new(AtomicBool::new(false)),
            game_checked: None,
            machine: real_machine(),
            pending: None,
            starting: None,
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

fn notice_or_changed(notice: Option<Notice>) -> Notice {
    notice.unwrap_or_else(changed_account)
}

fn changed_account() -> Notice {
    Notice::Error(tr("Your VRChat account changed: try again.").into())
}

/// Runs `send` on `client` only while it is still `account`'s session, under the one lock the send itself takes: a
/// sign-in that changed between the press and now never sends.
fn send_as<T>(client: &Mutex<Client>, account: &str, send: impl FnOnce(&mut Client) -> Result<T, Error>) -> Result<T, Notice> {
    let mut c = client.lock().unwrap();
    if c.signed_in_user_id() != Some(account) {
        return Err(changed_account());
    }
    send(&mut c).map_err(|e| Notice::Error(error_text(&e)))
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
    /// `expected` is the account the press was made on ([`Action::As`]); a press that names none sends nothing.
    fn launcher_sender(&self, expected: Option<&str>) -> Result<Arc<Mutex<Client>>, Notice> {
        let Some(client) = self.launcher_client() else {
            return Err(Notice::Error(tr("Sign in to VRChat in Settings to boop and ask for invites.").into()));
        };
        let account = client.lock().unwrap().signed_in_user_id().map(str::to_owned);
        match (&self.view().launcher.source, account, expected) {
            (Source::SignedIn { user_id, .. }, Some(account), Some(expected)) if *user_id == account && account == expected => Ok(client),
            _ => Err(changed_account()),
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
        let reopen;
        let source_changed;
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
            if changed || regranted {
                rt.epoch += 1;
            }
            source_changed = changed && rt.loaded_for.is_some();
            if changed {
                rt.failures = 0;
                rt.retry_at = None;
                // What one account was shown (or refused) is not another's: its caches, its accepted invites, its
                // destination waiting for a Mac's game.
                rt.worlds.clear();
                rt.lookup_failed.clear();
                rt.profiles.clear();
                rt.accepted.clear();
                rt.pending = None;
                // A cool-down taken by one account's press says nothing of another's.
                rt.cooldowns = Cooldowns::default();
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
        // Another source: the previous account's list (friends, invites, notes, the open profile) is not shown to it,
        // not even while its own list is read or failing to be read.
        if source_changed {
            self.update_if(|s| {
                let other = matches!(&s.launcher.source, Source::SignedIn { user_id, .. } if Some(user_id) != account.as_ref());
                if other {
                    let blank = LauncherState { game: s.launcher.game.clone(), possible: Possible { play: s.launcher.possible.play, ..Possible::default() }, loading: true, ..LauncherState::default() };
                    *s.launcher = blank;
                    return true;
                }
                s.launcher.profile.take().is_some()
            });
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
        let starting_profile = self.launcher.lock().unwrap().starting.filter(|(.., at)| at.elapsed() < STARTING_FOR);
        let look = here && (machine.by_hand() || !view.vrchat.runs.is_empty() || view.vrchat.running || starting_profile.is_some() || matches!(view.launcher.game, Game::Starting(_) | Game::Running(_)));
        let runs = if look { runs_with_profiles(machine, &view.vrchat.runs) } else { Vec::new() };
        // A start on a profile is over once a VRChat runs on it (or its minute passed).
        let starting_profile = starting_profile.filter(|(profile, ..)| !runs.iter().any(|r| r.2 == *profile));
        self.launcher.lock().unwrap().starting = starting_profile;
        // The accounts seen on each profile are remembered, so the choice says whose each one is.
        let learned: Vec<(u32, String)> = view
            .vrchat
            .runs
            .iter()
            .filter_map(|r| r.account.clone().filter(|a| !a.is_empty()).map(|a| (r.profile, a)))
            .filter(|(p, a)| view.settings.launch.profiles.get(p) != Some(a) && *p <= orbiters_core::vrchat_launch::PROFILE_MAX)
            .collect();
        if !learned.is_empty() {
            self.update_settings(|s| s.launch.profiles.extend(learned));
        }
        let profiles = if here && !machine.by_hand() { game_profiles(&self.view().settings.launch.profiles, &runs, &view.vrchat.runs, starting_profile.map(|(p, m, _)| (p, m))) } else { Vec::new() };
        let running: Vec<(u32, bool)> = runs.iter().map(|r| (r.0, r.1)).collect();
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
            let changed = s.launcher.game != game || s.launcher.possible.play != here || s.launcher.profiles != profiles;
            if s.launcher.profiles != profiles {
                s.launcher.profiles = profiles;
            }
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
        let patience = if self.launcher.lock().unwrap().machine.by_hand() { MAC_STARTING_FOR } else { STARTING_FOR };
        self.launcher_started.lock().unwrap().is_some_and(|t| t.elapsed() < patience)
    }

    /// The game started by hand is up: the destination that waited for it goes as an invite to oneself, which VRChat
    /// shows once the person is in. One that waited more than ten minutes is forgotten (the game was not started).
    fn launcher_go_pending(self: &Shared) {
        let Some((place, since, account)) = self.launcher.lock().unwrap().pending.take() else { return };
        if since.elapsed() > PENDING_FOR {
            return;
        }
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("launcher-pending".into()).spawn(move || {
            // To the account it was asked for only (the pending one is dropped at an account change, and checked here).
            let notice = ctl.launcher_invite_self(&place, account.as_deref());
            ctl.launcher_notice(notice.clone());
            std::thread::sleep(NOTICE_FOR);
            ctl.launcher_notice_over(&notice);
        });
    }

    fn launcher_invite_self(&self, place: &Place, account: Option<&str>) -> Notice {
        // Only for the account the press was made on, while its list is on show (AN-2).
        match (self.launcher_sender(account), account) {
            (Ok(client), Some(account)) => match send_as(&client, account, |c| c.invite_self(&place.world_id, &place.instance_id)) {
                Ok(()) => Notice::Sent { to: place.world_id.clone(), action: SentKind::Join },
                Err(notice) => notice,
            },
            _ if self.launcher_client().is_some() => changed_account(),
            _ => Notice::Error(tr("Sign in to VRChat in Settings so an invite to this instance can be sent to you.").into()),
        }
    }

    fn launcher_read_local(&self) {
        let Ok(mut state) = orbiters_core::launcher_local::read_now() else {
            // Caught while VRCX / VRCNext write their copy: the list on show stays if it is a local copy; again in a few
            // seconds. A list read with a VRChat sign-in that is over goes at once (its private details with it).
            {
                let mut rt = self.launcher.lock().unwrap();
                rt.loaded_for = Some(None);
                rt.retry_at = Some(Instant::now() + LOCAL_RETRY);
            }
            self.update_if(|s| {
                let signed = matches!(s.launcher.source, Source::SignedIn { .. });
                if signed {
                    let kept = LauncherState { game: s.launcher.game.clone(), possible: Possible { play: s.launcher.possible.play, ..Possible::default() }, ..LauncherState::default() };
                    *s.launcher = kept;
                }
                signed
            });
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
            state.profiles = s.launcher.profiles.clone();
            // The account's own profile is read on its own (`launcher_mine`): kept for the same account only.
            if s.launcher.source == state.source {
                state.mine = s.launcher.mine.clone();
            }
            // An open profile stays only when the source is the same copy (never from a VRChat sign-in into a local one).
            let same_source = matches!((&s.launcher.source, &state.source), (Source::Database { app: a, .. }, Source::Database { app: b, .. }) if a == b);
            state.profile = s.launcher.profile.clone().filter(|p| same_source && state.friends.iter().any(|f| f.id == p.friend.id));
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
                    state.invites.retain(|i| !rt.accepted.iter().any(|a| a.invite.id == i.id));
                    let dismissed = rt.dismissed.get_or_insert_with(load_dismissed);
                    state.boops.retain(|b| !dismissed.contains_key(&b.id));
                    for f in &mut state.friends {
                        if let Some(d) = rt.worn.get(&f.id) {
                            apply_pictures(&rt.pictures, d, &mut f.look);
                        }
                    }
                    rt.pipeline.is_none()
                };
                self.launcher_commit_read(state);
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

    /// A full read's list into the state: published only when it differs from what is on show.
    pub(super) fn launcher_commit_read(&self, mut state: LauncherState) {
        self.update_if(|s| {
            state.game = s.launcher.game.clone();
            state.possible.play = s.launcher.possible.play;
            // An open profile stays only when the list on show was this same account's, and the person is in it.
            let same_account = s.launcher.source == state.source;
            state.profile = s.launcher.profile.clone().filter(|p| same_account && (state.friends.iter().any(|f| f.id == p.friend.id) || state.me.as_ref().is_some_and(|m| m.id == p.friend.id)));
            state.notice = s.launcher.notice.clone();
            state.loading = false;
            // The scheduled read that found nothing new publishes nothing.
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
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
        self.launcher_apply(Some(from), message);
    }

    /// One pipeline message applied to the list.
    pub(super) fn launcher_message(&self, message: &str) {
        self.launcher_apply(None, message);
    }

    /// Applies `message`; with `from`, only while that is still the launcher's pipeline. The check and the write happen
    /// under the launcher's lock held throughout, so a pipeline replaced meanwhile (another account, a new sign-in)
    /// cannot write into the new list between the two.
    fn launcher_apply(&self, from: Option<&Arc<AtomicBool>>, message: &str) {
        let mut rt = self.launcher.lock().unwrap();
        if let Some(from) = from {
            if !rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, from)) || from.load(Ordering::Acquire) {
                return;
            }
        }
        let dismissed: Vec<String> = rt.dismissed.get_or_insert_with(load_dismissed).keys().cloned().collect();
        // A clear or hide from VRChat (another device) also ends an accepted invite waiting here: its card never comes
        // back. Seen by applying the message to the accepted ones alone.
        if !rt.accepted.is_empty() {
            let mut probe = LauncherState { invites: rt.accepted.iter().map(|a| a.invite.clone()).collect(), ..LauncherState::default() };
            launcher_feed::apply(&mut probe, &[], message, now());
            let left: Vec<String> = probe.invites.iter().map(|i| i.id.clone()).collect();
            rt.accepted.retain(|a| left.contains(&a.invite.id));
        }
        let accepted: Vec<String> = rt.accepted.iter().map(|a| a.invite.id.clone()).chain(rt.hiding.iter().cloned()).collect();
        let groups = rt.favourite_groups.clone();
        let mut applied = None;
        self.update_if(|s| {
            let mut state = (*s.launcher).clone();
            let a = launcher_feed::apply(&mut state, &groups, message, now());
            state.boops.retain(|b| !dismissed.contains(&b.id));
            // An invite accepted here and on the way does not come back with a repeated notification.
            state.invites.retain(|i| !accepted.contains(&i.id));
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            applied = Some(a);
            changed
        });
        for (id, d) in applied.unwrap_or_default().decorations {
            rt.worn.insert(id, d);
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
        // The account the press was made on (the page names it); none, or one bound twice: nothing is sent for it.
        let (account, action) = match action {
            Action::As { account, action } if !matches!(*action, Action::As { .. }) => (Some(account), *action),
            Action::As { .. } => return Some(changed_account()),
            action => (None, action),
        };
        let account = account.as_deref();
        match action {
            Action::Play { mode, destination } => Some(self.launcher_play(mode, destination, account)),
            Action::PlayOn { mode, profile } => Some(self.launcher_play_on(mode, profile)),
            // The account's own profile: REQUEST15 7b.
            Action::EditProfile(_) | Action::ListPictures(_) => None,
            Action::AcceptInvite { invite, mode } => {
                // An invite is the account's whose list showed it: never accepted for another, nor with no sign-in.
                let owner = match (self.launcher_sender(account), account) {
                    (Ok(_), Some(owner)) => owner.to_string(),
                    (sender, _) => return Some(notice_or_changed(sender.err())),
                };
                let found = self.view().launcher.invites.iter().find(|i| i.id == invite).cloned();
                let Some(found) = found.filter(|i| i.place.is_some()) else { return Some(Notice::Error(tr("This invite is no longer here.").into())) };
                // One accept per invite: reserved before anything starts (two presses, two threads).
                {
                    let from = match &self.view().launcher.me {
                        Some(me) => match &me.whereabouts {
                            Whereabouts::World(p) => Some((p.world_id.clone(), p.instance_id.clone())),
                            _ => None,
                        },
                        None => None,
                    };
                    let mut rt = self.launcher.lock().unwrap();
                    if rt.accepted.iter().any(|a| a.invite.id == invite) {
                        return Some(Notice::Error(tr("This invite is no longer here.").into()));
                    }
                    rt.accepted.push(Accepted { invite: found.clone(), at: Instant::now(), owner: owner.clone(), from, hide_failures: 0, hide_again: None, moved: false });
                }
                let notice = self.launcher_play(mode, found.place.clone(), Some(&owner));
                if matches!(notice, Notice::Error(_)) {
                    // Not on the way: the reservation goes, the card stays.
                    self.launcher.lock().unwrap().accepted.retain(|a| a.invite.id != invite);
                } else {
                    // The card goes here at once; VRChat hides the invite only once the account got there (a start that
                    // fails, or lands at home, leaves it in VRChat and brings the card back).
                    self.update(|s| s.launcher.invites.retain(|i| i.id != invite));
                }
                Some(notice)
            }
            Action::Dismiss { id } => {
                self.launcher_dismiss(&id, account);
                None
            }
            Action::RequestInvite { to } => Some(self.launcher_send(false, &to, None, account)),
            Action::Boop { to, emoji } => Some(self.launcher_send(true, &to, emoji.as_deref(), account)),
            Action::As { .. } => Some(changed_account()),
            Action::OpenProfile { id } => {
                self.launcher_profile(&id, account);
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

    fn launcher_play(self: &Shared, mode: LaunchMode, destination: Option<Place>, account: Option<&str>) -> Notice {
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        // Ids come from the network (a friend's location, an invite) or over IPC: only VRChat's own grammar goes on.
        if destination.as_ref().is_some_and(|p| !orbiters_core::vrchat_launch::is_destination(&p.world_id, &p.instance_id)) {
            return Notice::Error(tr("VRChat does not know this world.").into());
        }
        // A destination from a signed-in account's list (a friend's room, an invite) goes only while that account is
        // the one signed in and on show: checked here and again right before the hand-off.
        let scoped = destination.is_some() && account.is_some();
        if scoped && self.launcher_sender(account).is_err() {
            return changed_account();
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        // One start at a time: held from the look at the running VRChats to the start, so that two presses (or a press
        // and an invite's Accept, on their own threads) never start two; and none while one is starting.
        let mut started = self.launcher_started.lock().unwrap();
        let running = machine.running();
        let settings = self.view().settings.launch.clone();
        if !running.is_empty() {
            // Never a second VRChat: the running one goes there.
            let Some(place) = destination else { return Notice::Error(tr("VRChat is already running.").into()) };
            if machine.by_hand() {
                drop(started);
                return self.launcher_invite_self(&place, account);
            }
            let travel: Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| Arc::new(super::travel::Real));
            let bridge = super::travel::Bridge { main: self.sides.main_pid(), sides: self.sides.side_pids() };
            // Only the bridge's side sessions run: travel starts the person's own VRChat. That start is under the same
            // start lock (two Joins at once start one), and none while one is starting.
            // The route is decided once, from one look at the processes, and that route is the one taken.
            let processes = travel.running();
            let chosen = super::travel::route(false, &processes, &bridge, |p| travel.own_osc_ports(p));
            if chosen == super::travel::Route::Start && started.is_some_and(|t| t.elapsed() < STARTING_FOR) {
                return Notice::Error(tr("Starting VRChat…").into());
            }
            if scoped && self.launcher_sender(account).is_err() {
                return changed_account();
            }
            let went = super::travel::go_on(travel.as_ref(), chosen, &processes, &bridge, &place.world_id, &place.instance_id);
            if went == Ok(super::travel::Went::Started) {
                *started = Some(Instant::now());
            }
            drop(started);
            return match went {
                Ok(_) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                Err(super::travel::GoFailure::Invite(_)) => match (self.launcher_sender(account), account) {
                    (Ok(client), Some(account)) => match send_as(&client, account, |c| c.invite_self(&place.world_id, &place.instance_id)) {
                        Ok(()) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                        Err(notice) => notice,
                    },
                    _ if self.launcher_client().is_some() => changed_account(),
                    _ => Notice::Error(tr("VRChat did not take the link. Sign in to VRChat in Settings so an invite can be sent instead.").into()),
                },
                Err(super::travel::GoFailure::BridgeBusy) => Notice::Error(tr("Only the assistant's VRChat is running. Close it first.").into()),
                Err(super::travel::GoFailure::Start(words)) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
                Err(super::travel::GoFailure::BadDestination) => Notice::Error(tr("VRChat does not know this world.").into()),
            };
        }
        if machine.by_hand() {
            return self.launcher_play_by_hand(machine.as_ref(), destination, account, &mut started);
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
        if scoped && self.launcher_sender(account).is_err() {
            return changed_account();
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

    /// Home's Play with a profile chosen (REQUEST15 7a): a VRChat on `profile`, beside the ones running on other profiles
    /// (VRChat's own launcher with `--profile`), never a second one on the same profile. Through Steam when none runs.
    fn launcher_play_on(self: &Shared, mode: LaunchMode, profile: u32) -> Notice {
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        if profile > orbiters_core::vrchat_launch::PROFILE_MAX {
            return Notice::Error(tr("VRChat does not know this profile.").into());
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        if machine.by_hand() {
            // A Mac: Obelisk runs one VRChat, on its own profile.
            return if profile == 0 { self.launcher_play(mode, None, None) } else { Notice::Error(tr("On a Mac, VRChat runs one profile, through Obelisk.").into()) };
        }
        // The same start lock as Play: from the look at the running VRChats to the start, two presses start one.
        let mut started = self.launcher_started.lock().unwrap();
        let view = self.view();
        let runs = runs_with_profiles(machine.as_ref(), &view.vrchat.runs);
        if runs.iter().any(|r| r.2 == profile) {
            return Notice::Error(tr!("VRChat already runs on profile {n}: choose another profile to start one more.", n = profile));
        }
        let starting = self.launcher.lock().unwrap().starting.filter(|(.., at)| at.elapsed() < STARTING_FOR);
        if starting.is_some() || (runs.is_empty() && started.is_some_and(|t| t.elapsed() < STARTING_FOR)) {
            return Notice::Error(tr("Starting VRChat…").into());
        }
        if !machine.installed() {
            return Notice::Error(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into());
        }
        let settings = view.settings.launch.clone();
        let mut args = orbiters_core::vrchat_launch::arguments(mode, &settings, None);
        // VRChat's saved window is one for all its profiles: the same care as Play's.
        let saved = machine.saved_window();
        let mut keep_window = None;
        match mode {
            LaunchMode::Desktop => args.extend(desktop_window(saved, settings.desktop_window)),
            LaunchMode::Vr if runs.is_empty() && settings.performance && settings.small_window_in_vr => {
                keep_window = saved.filter(SavedWindow::normal).filter(|w| settings.desktop_window != Some(*w))
            }
            LaunchMode::Vr => {}
        }
        args.extend(orbiters_core::vrchat_launch::profile_arg(profile));
        let result = if runs.is_empty() { machine.start(&args) } else { machine.start_beside(&args) };
        match result {
            Ok(()) => {
                *started = Some(Instant::now());
                drop(started);
                self.launcher.lock().unwrap().starting = Some((profile, mode, Instant::now()));
                self.update(|s| {
                    if runs.is_empty() {
                        s.launcher.game = Game::Starting(mode);
                    }
                    match s.launcher.profiles.iter_mut().find(|p| p.profile == profile) {
                        Some(p) => p.starting = Some(mode),
                        None => {
                            s.launcher.profiles.push(orbiters_core::launcher::GameProfile { profile, starting: Some(mode), ..Default::default() });
                            s.launcher.profiles.sort_by_key(|p| p.profile);
                        }
                    }
                });
                if settings.last_mode != mode || keep_window.is_some() {
                    self.update_settings(|s| {
                        s.launch.last_mode = mode;
                        if keep_window.is_some() {
                            s.launch.desktop_window = keep_window;
                        }
                    });
                }
                Notice::Sent { to: String::new(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Accepted invites: hidden at VRChat once their own account is in their instance (having moved there after the
    /// press); a hide that failed is tried again a minute later. After [`ARRIVE_WITHIN`] without arriving, the card comes
    /// back, in its own account's list only.
    fn launcher_settle_accepted(&self, client: Option<&Arc<Mutex<Client>>>) {
        let view = self.view();
        let shown = match &view.launcher.source {
            Source::SignedIn { user_id, .. } => Some(user_id.clone()),
            _ => None,
        };
        let here = match &view.launcher.me {
            Some(me) => match &me.whereabouts {
                Whereabouts::World(p) => Some((p.world_id.clone(), p.instance_id.clone())),
                _ => None,
            },
            None => None,
        };
        let (hide, back) = {
            let mut rt = self.launcher.lock().unwrap();
            if rt.accepted.is_empty() {
                return;
            }
            let arrived = |a: &Accepted| a.invite.place.as_ref().is_some_and(|p| here.as_ref().is_some_and(|(w, i)| *w == p.world_id && *i == p.instance_id)) && (a.from != here || a.moved);
            let mut hide = Vec::new();
            let mut back = Vec::new();
            for mut a in std::mem::take(&mut rt.accepted) {
                if here.is_some() && a.from != here {
                    a.moved = true;
                }
                if shown.as_deref() != Some(a.owner.as_str()) {
                    // Another account's list is on show: nothing of this one's is hidden or shown with it (an account
                    // change drops them all; this covers a list still being replaced).
                    rt.accepted.push(a);
                } else if a.hide_failures > 0 || arrived(&a) {
                    if a.hide_again.is_none_or(|t| Instant::now() >= t) {
                        hide.push(a);
                    } else {
                        rt.accepted.push(a);
                    }
                } else if a.at.elapsed() > ARRIVE_WITHIN {
                    back.push(a);
                } else {
                    rt.accepted.push(a);
                }
            }
            // While their hide is on the way they still count as accepted (a repeated notification stays out).
            rt.hiding = hide.iter().map(|a: &Accepted| a.invite.id.clone()).collect();
            (hide, back)
        };
        for mut a in hide {
            // With the owner's own session only, under the lock the hide takes.
            let done = client.is_some_and(|client| {
                send_as(client, &a.owner, |c| match c.hide_notification(&a.invite.id) {
                    Err(Error::NotFound) => Ok(()),
                    other => other,
                })
                .is_ok()
            });
            if !done {
                a.hide_failures += 1;
                a.hide_again = Some(Instant::now() + HIDE_AGAIN);
                if a.hide_failures < HIDE_TRIES {
                    self.launcher.lock().unwrap().accepted.push(a);
                } else {
                    eprintln!("launcher: an accepted invite could not be hidden at VRChat after {HIDE_TRIES} tries"); // i18n: not shown (a log line)
                }
            }
        }
        self.launcher.lock().unwrap().hiding.clear();
        if !back.is_empty() {
            self.update(|s| {
                for a in back {
                    // Its own account's list only (checked again: the list may have changed meanwhile).
                    if !matches!(&s.launcher.source, Source::SignedIn { user_id, .. } if *user_id == a.owner) {
                        continue;
                    }
                    if !s.launcher.invites.iter().any(|i| i.id == a.invite.id) {
                        s.launcher.invites.push(a.invite);
                    }
                }
                s.launcher.invites.sort_by_key(|i| std::cmp::Reverse(i.received_at));
            });
        }
    }

    /// A Mac: Obelisk is opened for the person to press Play in; the destination waits for the game.
    /// `started` is the start's lock, held by [`Self::launcher_play`].
    fn launcher_play_by_hand(self: &Shared, machine: &dyn Machine, destination: Option<Place>, account: Option<&str>, started: &mut Option<Instant>) -> Notice {
        if machine.engine_missing() {
            let _ = machine.open_engine();
            return Notice::Error(tr("VRChat runs on a Mac through Obelisk, a free app. Get it, turn on Beta versions in its settings, then download VRChat in it.").into());
        }
        if !machine.installed() {
            let _ = machine.open_engine();
            return Notice::Error(tr("Download VRChat in Obelisk (with Beta versions on in its settings), then come back.").into());
        }
        // A second Play while the person is still to press Play in Obelisk opens nothing again, and keeps the
        // destination the first asked for (a plain Play asks for none); a new destination replaces it.
        let again = started.is_some_and(|t| t.elapsed() < MAC_STARTING_FOR);
        if again {
            if let Some(p) = destination.clone() {
                self.launcher.lock().unwrap().pending = Some((p, Instant::now(), account.map(str::to_owned)));
            }
            return Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch };
        }
        match machine.start(&[]) {
            Ok(()) => {
                // Bound to the account it was asked on.
                *started = Some(Instant::now());
                self.launcher.lock().unwrap().pending = destination.clone().map(|p| (p, Instant::now(), account.map(str::to_owned)));
                self.update(|s| s.launcher.game = Game::Starting(LaunchMode::Desktop));
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Hides an invite, a request or a boop here; an invite or a request is hidden at VRChat too (nobody is told).
    fn launcher_dismiss(&self, id: &str, account: Option<&str>) {
        // A press made on another account's list changes nothing of the list on show (nor the kept dismissals).
        let shown = match &self.view().launcher.source {
            Source::SignedIn { user_id, .. } => Some(user_id.clone()),
            _ => None,
        };
        if shown.is_some() && shown.as_deref() != account {
            return;
        }
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
            if let (Ok(client), Some(account)) = (self.launcher_sender(account), account) {
                let _ = send_as(&client, account, |c| c.hide_notification(id));
            }
        }
    }

    fn launcher_send(&self, boop: bool, to: &str, emoji: Option<&str>, account: Option<&str>) -> Notice {
        let kind = if boop { SentKind::Boop } else { SentKind::RequestInvite };
        // Refused before anything left: keyed, so that the page takes back its "Sent" for this person.
        let failed = |n: Notice| match n {
            Notice::Error(text) => Notice::Failed { to: to.into(), action: kind, text },
            other => other,
        };
        let (client, account) = match (self.launcher_sender(account), account) {
            (Ok(client), Some(account)) => (client, account),
            (Err(notice), _) => return failed(notice),
            (Ok(_), None) => return failed(changed_account()),
        };
        let at = now();
        let taken = self.launcher.lock().unwrap().cooldowns.take(boop, to, at);
        if let Err(until) = taken {
            return Notice::CoolingDown { to: to.into(), until };
        }
        // With the press's account only, checked under the lock the send takes.
        let sent = {
            let mut c = client.lock().unwrap();
            if c.signed_in_user_id() != Some(account) {
                drop(c);
                self.launcher.lock().unwrap().cooldowns.release(boop, to);
                return failed(changed_account());
            }
            if boop { c.boop(to, emoji) } else { c.request_invite(to) }
        };
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

    fn launcher_profile(&self, id: &str, account: Option<&str>) {
        // The row, the source it came from and this request, taken together under the launcher's lock: a press made on
        // another account's list opens nothing, and a later request (or account) makes this one write nothing.
        // The client and whose session it is are read first (never a client's lock under the launcher's).
        let client = self.launcher_client();
        let client_account = client.as_ref().and_then(|c| c.lock().unwrap().signed_in_user_id().map(str::to_owned));
        let (friend, epoch, request, client) = {
            let mut rt = self.launcher.lock().unwrap();
            let view = self.view();
            let shown = match &view.launcher.source {
                Source::SignedIn { user_id, .. } => Some(user_id.as_str()),
                _ => None,
            };
            if shown != account {
                return;
            }
            let Some(friend) = view.launcher.friend(id).cloned().or_else(|| view.launcher.me.clone().filter(|m| m.id == id)) else { return };
            let client = client.filter(|_| client_account.as_deref() == account);
            rt.profile_request += 1;
            let request = rt.profile_request;
            self.update(|s| s.launcher.profile = Some(Profile { friend: friend.clone(), loading: client.is_some(), ..Profile::default() }));
            (friend, rt.epoch, request, client)
        };
        let Some(client) = client else { return };
        let kept = self.launcher.lock().unwrap().profiles.get(id).filter(|(at, ..)| at.elapsed() < PROFILE_FOR).map(|(_, p, u, m)| (p.clone(), u.clone(), *m));
        let (public, user, mutuals) = match kept {
            Some(kept) => kept,
            None => {
                // Each call only while this profile is still the one open (clicking down a list asks for the last one).
                let wanted = || {
                    let rt = self.launcher.lock().unwrap();
                    rt.epoch == epoch && rt.profile_request == request && self.view().launcher.profile.as_ref().is_some_and(|p| p.friend.id == id)
                };
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
                if rt.epoch != epoch {
                    return;
                }
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
        // Checked and written under the launcher's lock: an account change in between writes nothing.
        let rt = self.launcher.lock().unwrap();
        if rt.epoch != epoch || rt.profile_request != request {
            return;
        }
        self.update(|s| {
            if s.launcher.profile.as_ref().is_some_and(|p| p.friend.id == profile.friend.id) {
                s.launcher.profile = Some(profile);
            }
        });
        drop(rt);
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
