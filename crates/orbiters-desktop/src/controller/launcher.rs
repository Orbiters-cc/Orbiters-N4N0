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
use orbiters_core::launcher::{Action, Game, LaunchMode, LauncherState, Notice, Place, Possible, Profile, SentKind, Source};
use orbiters_core::launcher_feed::{self, Cooldowns, Decorations};
use orbiters_core::vrchat_api::{Client, Error};
use orbiters_i18n::tr;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOCAL_EVERY: Duration = Duration::from_secs(120);
const FULL_EVERY: Duration = Duration::from_secs(30 * 60);
const GAME_EVERY: Duration = Duration::from_secs(5);
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);

/// The game on this computer, real or a test's.
pub trait Machine: Send + Sync {
    /// The VRChats running here: process id and whether it runs in Desktop mode (`--no-vr`).
    fn running(&self) -> Vec<(u32, bool)>;
    fn installed(&self) -> bool;
    /// Starts VRChat with these arguments (after `-applaunch 438100`).
    fn start(&self, args: &[String]) -> Result<(), String>;
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

pub struct RealMachine;

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
        Arc::new(RealMachine)
    }
}

impl Machine for RealMachine {
    fn running(&self) -> Vec<(u32, bool)> {
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::OnlyIfNotSet));
        system
            .processes()
            .values()
            .filter(|p| matches!(p.name().to_string_lossy().to_lowercase().as_str(), "vrchat.exe" | "vrchat"))
            .map(|p| (p.pid().as_u32(), p.cmd().iter().any(|a| a.to_string_lossy() == "--no-vr")))
            .collect()
    }

    fn installed(&self) -> bool {
        crate::bridge::win::steam_exe().and_then(|exe| exe.parent().map(|d| d.to_path_buf())).and_then(|steam| orbiters_core::vrchat_launch::vrchat_dir(&steam)).is_some()
            || crate::bridge::win::vrchat_launcher().is_some()
    }

    fn start(&self, args: &[String]) -> Result<(), String> {
        // Through Steam as a person does; without Steam, VRChat's own launcher with the link first (as it registers
        // itself), the options after: never reordered, `-screen-fullscreen 0` stays a pair.
        if crate::bridge::win::steam_exe().is_some() {
            crate::bridge::win::start_vrchat(args).map_err(|f| f.message)
        } else {
            crate::bridge::win::start_vrchat_beside(args).map_err(|f| f.message)
        }
    }
}

/// What the launcher keeps besides the published state.
pub(super) struct Runtime {
    /// The account the state was read for (`None`: the local databases).
    loaded_for: Option<Option<String>>,
    loaded_at: Option<Instant>,
    favourite_groups: Vec<(String, String)>,
    cooldowns: Cooldowns,
    /// Decoration template id → picture (`None` while unknown or failed).
    pictures: HashMap<String, Option<String>>,
    /// Which friend wears which decorations.
    worn: BTreeMap<String, Decorations>,
    /// World id → its name and picture (`None` once asked and not given).
    worlds: HashMap<String, Option<(String, String)>>,
    pipeline: Option<Arc<AtomicBool>>,
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
            favourite_groups: Vec::new(),
            cooldowns: Cooldowns::default(),
            pictures: HashMap::new(),
            worn: BTreeMap::new(),
            worlds: HashMap::new(),
            pipeline: None,
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
            let every = if account.is_some() { FULL_EVERY } else { LOCAL_EVERY };
            let due = changed || rt.resync.swap(false, Ordering::AcqRel) || rt.loaded_at.is_none_or(|t| t.elapsed() > every);
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

    fn launcher_game(self: &Shared, machine: &dyn Machine) {
        // A Mac runs its own VRChat through Obelisk whatever it follows on the network; where Obelisk is not, a Mac
        // that follows a VRChat elsewhere is not told to get it.
        let remote = self.view().vrchat.remote;
        let here = if machine.by_hand() { !(remote && machine.engine_missing()) } else { orbiters_core::osc::VRCHAT_RUNS_HERE && !remote };
        let running = if here { machine.running() } else { Vec::new() };
        let game = match running.first() {
            Some((_, desktop)) => Game::Running(if *desktop { LaunchMode::Desktop } else { LaunchMode::Vr }),
            None if !here => Game::NotRunning,
            None => match self.view().launcher.game {
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
        self.update(|s| {
            if s.launcher.game != game {
                s.launcher.game = game;
            }
            if s.launcher.possible.play != here {
                s.launcher.possible.play = here;
            }
        });
    }

    fn launcher_started_recently(&self) -> bool {
        // A game the person starts by hand in Obelisk takes its sign-in, an update check and Easy Anti-Cheat first.
        let patience = if self.launcher.lock().unwrap().machine.by_hand() { 180 } else { 60 };
        self.launcher_started.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(patience))
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
        match self.launcher_client() {
            Some(client) => match client.lock().unwrap().invite_self(&place.world_id, &place.instance_id) {
                Ok(()) => Notice::Sent { to: place.world_id.clone(), action: SentKind::Join },
                Err(e) => Notice::Error(error_text(&e)),
            },
            None => Notice::Error(tr("Sign in to VRChat in Settings so an invite to this instance can be sent to you.").into()),
        }
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
                    rt.favourite_groups = groups;
                    rt.worn = worn;
                    rt.cooldowns.mark(&mut state.friends, now());
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
                    // Tried: the next try after the usual wait, not every two seconds.
                    let mut rt = self.launcher.lock().unwrap();
                    rt.loaded_at = Some(Instant::now());
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
                || client.lock().unwrap().pipeline_token().ok().map(|s| s.0),
                |message| {
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
                    self.launcher_dismiss(&invite);
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
                self.launcher.lock().unwrap().loaded_at = None;
                None
            }
        }
    }

    fn launcher_play(self: &Shared, mode: LaunchMode, destination: Option<Place>) -> Notice {
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        let running = machine.running();
        let settings = self.view().settings.launch.clone();
        if !running.is_empty() {
            // Never a second VRChat: the running one goes there.
            let Some(place) = destination else { return Notice::Error(tr("VRChat is already running.").into()) };
            if machine.by_hand() {
                return self.launcher_invite_self(&place);
            }
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
        if machine.by_hand() {
            return self.launcher_play_by_hand(machine.as_ref(), destination);
        }
        if !machine.installed() {
            return Notice::Error(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into());
        }
        let args = orbiters_core::vrchat_launch::arguments(mode, &settings, destination.as_ref().map(|p| (p.world_id.as_str(), p.instance_id.as_str())));
        match machine.start(&args) {
            Ok(()) => {
                *self.launcher_started.lock().unwrap() = Some(Instant::now());
                self.update(|s| s.launcher.game = Game::Starting(mode));
                if settings.last_mode != mode {
                    self.update_settings(|s| s.launch.last_mode = mode);
                }
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// A Mac: Obelisk is opened for the person to press Play in; the destination waits for the game.
    fn launcher_play_by_hand(self: &Shared, machine: &dyn Machine, destination: Option<Place>) -> Notice {
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
                *self.launcher_started.lock().unwrap() = Some(Instant::now());
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
