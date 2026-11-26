//! The Orbiters account in the tray service. It is the one the Unity tools keep (`orbiters_core::account`): read at
//! start, checked with Orbiters, and followed while the app runs, so signing in or out in MCB or My Avatar does the same
//! here within seconds. Signing in runs the tools' browser pairing: a listener on this computer's loopback waits for the
//! browser to bring the code of the website's Connect, and trades it for the credential. The credential stays in this
//! process (the window only sees the account's name and state).

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use super::rest::{sooner, until};
use super::{Controller, NoticeKind, Shared};
use crate::state::AccountState;
use orbiters_core::account::{self, Account, Api, Callback, Check, Exchange, Served, Server};
use orbiters_i18n::tr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How long a browser sign-in waits for the website (the server's own link lifetime).
const LINK_LIFETIME: Duration = Duration::from_secs(600);
/// An account that could not be checked (offline) is checked again this often.
const RECHECK_OFFLINE: Duration = Duration::from_secs(60);
/// A refused credential is asked about again after this long, twice as long each time after, at most
/// [`RECHECK_REFUSED_MAX`]: a refusal by something in front of Orbiters (a deploy, a firewall page) does not last until
/// the next start. Meanwhile it stays refused: only a valid answer signs in again.
const RECHECK_REFUSED: Duration = Duration::from_secs(600);
const RECHECK_REFUSED_MAX: Duration = Duration::from_secs(6 * 3600);

/// An account file that could not be read is tried again at once this many times, this far apart (another tool may be
/// writing it), then every [`REREAD`].
const READ_TRIES: u32 = 5;
const READ_APART: Duration = Duration::from_millis(100);
const REREAD: Duration = Duration::from_secs(2);

/// What the account shows while its file cannot be read.
fn unreadable_text() -> &'static str {
    tr("Your Orbiters account could not be read on this computer, so you are signed out for now. Orbiters keeps trying; if this stays, sign in again.")
}

/// The same, where it was read before and stays in use for what is under way (a call).
fn unreadable_now_text() -> &'static str {
    tr("Your Orbiters account could not be read on this computer just now. Orbiters keeps trying; if this stays, sign in again.")
}

/// When a credential refused `refusals` times in a row is asked about again.
fn refused_recheck(refusals: u32) -> Duration {
    RECHECK_REFUSED.saturating_mul(1 << refusals.clamp(1, 16).saturating_sub(1)).min(RECHECK_REFUSED_MAX)
}

/// What the service knows besides the snapshot.
#[derive(Default)]
pub struct AccountRuntime {
    /// The signed-in account (with its credential), while one is saved for the current server.
    pub(super) account: Option<Account>,
    /// The server and the account file's change time last read, to notice another tool signing in or out.
    pub(super) read: Option<(Server, Option<SystemTime>)>,
    /// The saved account could not be read, or the environment is being switched and the next one's is not read yet: who it
    /// is is unknown (controller/identity.rs).
    pub(super) unknown: bool,
    /// The file that could not be read, its change time then, and when that was.
    unreadable: Option<(Server, Option<SystemTime>, Instant)>,
    /// The service just started: the account's file is not read yet.
    pub(super) starting: bool,
    signing_in: Option<SignIn>,
    /// Bumped by every sign-in, cancel and sign-out, so an answer for an older attempt is dropped.
    generation: u64,
    offline_since: Option<Instant>,
    /// The credential was refused: when, and how many times in a row.
    refused: Option<(Instant, u32)>,
    /// Whose all this is, and its epoch (controller/identity.rs).
    pub(super) epochs: super::identity::Epochs,
}

/// Held while this process reads the saved account's file to write it again, or removes it.
static ACCOUNT_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A sign-in waiting for the browser: the page it opened, and the switch that closes its listener.
struct SignIn {
    server: Server,
    link_url: String,
    generation: u64,
    stop: Arc<AtomicBool>,
    /// What the account showed before (put back when the sign-in ends without one).
    before: AccountState,
}

impl AccountRuntime {
    /// Whose all this is; `None` while nothing was read at all.
    pub(super) fn identity(&self) -> Option<super::identity::Identity> {
        use super::identity::Identity;
        if self.unknown {
            return Some(Identity::Unknown { key: None });
        }
        self.read.map(|(server, _)| Identity::of(server, self.account.as_ref()))
    }

    pub(super) fn signed_as(&self) -> orbiters_n4n0::status::SignedAs {
        if self.starting {
            return orbiters_n4n0::status::SignedAs::Unknown;
        }
        self.identity().map_or(orbiters_n4n0::status::SignedAs::Nobody, |i| i.signed_as())
    }

    /// `account` (or nobody) is what `server`'s file said, changed last at `modified`.
    fn settled(&mut self, server: Server, modified: Option<SystemTime>, account: Option<Account>) {
        self.read = Some((server, modified));
        self.account = account;
        self.unknown = false;
        self.unreadable = None;
        self.starting = false;
        self.offline_since = None;
        self.refused = None;
    }

    /// After a panic with it (`locks::StartsOver`): nobody is installed and who it is is unknown, until the account's file
    /// is read again. The old generation and any sign-in that was in progress are discarded.
    pub(super) fn start_over(&mut self) -> u64 {
        self.signing_in = None;
        self.unreadable = None;
        self.unsettled();
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Nobody is installed, and who it is is unknown: no credential is kept for any server.
    fn unsettled(&mut self) {
        self.read = None;
        self.account = None;
        self.unknown = true;
        self.starting = false;
        self.offline_since = None;
        self.refused = None;
    }

    /// What the account shows when no sign-in waits: signed in with the account saved, or signed out.
    fn resting(&self) -> AccountState {
        self.account.as_ref().map_or(AccountState::SignedOut, |a| AccountState::SignedIn { username: a.username.clone().unwrap_or_default() })
    }
}

impl Drop for SignIn {
    /// Cancelled, replaced, signed out, ended: its listener goes.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The account's file was read: what was said of one that could not be goes.
fn readable_again(s: &mut crate::state::Snapshot) {
    s.account.unreadable = false;
    if [unreadable_text(), unreadable_now_text()].into_iter().any(|t| s.account.error.as_deref() == Some(t)) {
        s.account.error = None;
    }
}

/// Opens a web page (http or https only) in the default browser.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let lower = url.to_ascii_lowercase();
    anyhow::ensure!(lower.starts_with("https://") || lower.starts_with("http://"), "{}", tr!("Not a web page: {url}", url = url));
    #[cfg(windows)]
    open_with_browser(url)?;
    #[cfg(target_os = "linux")]
    crate::linux::open::url(url)?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    Ok(())
}

/// The shell's "open" of a web address: the browser registered for http(s). Unlike url.dll's FileProtocolHandler,
/// which takes any path or protocol, only web addresses get here.
#[cfg(windows)]
fn open_with_browser(url: &str) -> anyhow::Result<()> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // SAFETY: plain calls with valid, null-terminated strings; COM is released only when this call initialised it.
    unsafe {
        // The shell may hand the address to COM handlers: initialise COM on this thread when it is not yet.
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if com.is_ok() {
            CoUninitialize();
        }
        // Values above 32 mean success.
        anyhow::ensure!(result.0 as usize > 32, "{}", tr!("Could not open the browser (error {code}).", code = result.0 as usize));
    }
    Ok(())
}

/// How often the account's file is looked at where the system does not tell of its changes, and the shortest time between two looks.
const LOOK_EVERY: Duration = Duration::from_secs(2);
const LOOK_APART: Duration = Duration::from_millis(500);

impl Controller {
    /// Reads the account now, then follows it (and a sign-in in progress) on a thread of its own.
    pub(super) fn start_account(self: &Shared) {
        // What the saved account is shows before any window asks (r21): "checking", never "not signed in" while nothing
        // was read yet. Its file only; Orbiters is asked on the thread.
        let server = Server::current();
        self.account_runtime.locked().starting = true;
        if let Ok(saved) = account::read_saved(server) {
            self.account_saved_at_launch(server, saved);
        }
        // The file changes when a tool signs in or out: the system says so, else it is looked at now and then. The folder is both
        // environments'.
        // An environment change also wakes this wait.
        let rest = self.rest("account", |_| ());
        let weak = std::sync::Arc::downgrade(self);
        let folder: Vec<_> = account::account_path(server).and_then(|p| Some(p.parent()?.to_path_buf())).into_iter().collect();
        let told = orbiters_core::folder_watch::watch("account-folder", folder, move || {
            if let Some(ctl) = weak.upgrade() {
                ctl.rests.wake("account");
            }
        });
        let ctl = self.clone();
        std::thread::Builder::new().name("account".into()).spawn(move || while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            ctl.account_tick();
            rest.wait(ctl.account_rests(told));
        }).or_log("a thread could not be started");
    }

    /// How long until the next look at the account: until something is due by the clock (a file that could not be read, Orbiters
    /// out of reach, a refused credential, what the assistant may do), and the file's own pace where nothing tells of it.
    fn account_rests(&self, told: bool) -> Option<Duration> {
        let mut due = (!told).then_some(LOOK_EVERY);
        {
            let rt = self.account_runtime.locked();
            if rt.unknown && rt.unreadable.is_none() {
                due = sooner(due, Some(LOOK_APART));
            }
            due = sooner(due, rt.unreadable.map(|(_, _, at)| until(Some(at), REREAD)));
            due = sooner(due, rt.offline_since.map(|at| until(Some(at), RECHECK_OFFLINE)));
            due = sooner(due, rt.refused.map(|(at, n)| until(Some(at), refused_recheck(n))));
        }
        due = sooner(due, self.assistant_access_wait());
        // (Never in a row: one due but still being asked is looked at again at the old pace.)
        due.map(|due| due.max(LOOK_APART))
    }

    /// A sign-in ended while it was replacing a saved account's check.
    /// Check the file again even when its contents stayed the same.
    fn check_restored_account(&self, rt: &mut AccountRuntime, before: &AccountState) {
        if matches!(before, AccountState::Checking { .. }) {
            rt.read = rt.read.map(|(server, _)| (server, Some(SystemTime::UNIX_EPOCH)));
            self.rests.wake("account");
        }
    }

    fn account_tick(self: &Shared) {
        let server = Server::current();
        let modified = account::modified(server);
        let (changed, offline_due, refused_due) = {
            let rt = self.account_runtime.locked();
            // One that could not be read: again now and then, or as soon as it changes.
            let waits = rt.unreadable.is_some_and(|(s, m, at)| (s, m) == (server, modified) && at.elapsed() < REREAD);
            (
                (rt.read != Some((server, modified)) || rt.unknown) && !waits,
                rt.offline_since.is_some_and(|t| t.elapsed() > RECHECK_OFFLINE),
                rt.refused.is_some_and(|(t, n)| t.elapsed() > refused_recheck(n)),
            )
        };
        if changed || offline_due || refused_due {
            self.read_account(server, modified, !changed && refused_due);
        }
        self.assistant_access_refresh();
    }

    /// What the account's file says at launch, before the account thread installed it.
    pub(super) fn account_saved_at_launch(&self, server: Server, saved: Option<Account>) {
        self.vrchat_login.locked().at_launch = super::vrchat_login::Owner::of(&super::identity::Identity::of(server, saved.as_ref()));
        self.account_at_launch(server, saved.map(|a| a.username));
    }

    /// The account as its file says at launch (`Some(username)`: one is saved), while it is still `Unknown`.
    pub(super) fn account_at_launch(&self, server: Server, saved: Option<Option<String>>) {
        self.update_if(|s| {
            if s.account.state != AccountState::Unknown {
                return false;
            }
            s.account.development = server == Server::Development;
            s.account.state = match saved {
                Some(username) => AccountState::Checking { username },
                None => AccountState::SignedOut,
            };
            true
        });
    }

    /// Reads the saved account for `server` and asks Orbiters whether it still works. `again_refused`: asking again
    /// about a credential that was refused, which shows as refused until an answer says otherwise.
    fn read_account(self: &Shared, server: Server, modified: Option<SystemTime>, again_refused: bool) {
        self.read_account_from(server, modified, again_refused, || {
            let mut tries = 1;
            loop {
                match account::read_saved(server) {
                    Err(_) if tries < READ_TRIES => {
                        tries += 1;
                        std::thread::sleep(READ_APART);
                    }
                    read => return read,
                }
            }
        });
    }

    /// The environment was switched (settings): nobody is signed in on the next one until its own account is read, and
    /// nothing of the last one's credential stays (verify3-r21 B2).
    pub(super) fn account_environment_changed(&self) {
        self.leave_unless_same(None);
        self.install_account(|rt| {
            // A read under way of the last environment is not installed.
            rt.generation += 1;
            self.account_generation.store(rt.generation, Ordering::SeqCst);
            rt.signing_in = None;
            rt.unreadable = None;
            rt.unsettled();
        });
    }

    /// [`Controller::read_account`] with the saved account read by `read` (tests: no file of the person's).
    pub(super) fn read_account_from(self: &Shared, server: Server, modified: Option<SystemTime>, again_refused: bool, read: impl FnOnce() -> anyhow::Result<Option<Account>>) {
        self.read_account_with(server, modified, again_refused, read, |token| Api::new(server).check(token));
    }

    /// The one gate every change of the signed-in identity (the server and the credential) goes through, BEFORE the new
    /// one is installed: the account file read (another tool signed in or out, the environment switched), a browser
    /// sign-in, a sign-out. When `next` is not the identity installed now, the account installed now leaves first
    /// (`calls_leave`: its call ends here, its media stops, nothing of it or of its shares stays shown), with its own
    /// credential for what Orbiters hears. (N4N0's conversation goes as the next one is installed, and by the member, not
    /// the credential: [`Controller::install_account`].) The same identity read again (a same-credential refresh) leaves
    /// nothing. True when the account left.
    pub(super) fn leave_unless_same(&self, next: Option<(Server, &str)>) -> bool {
        let held = {
            let rt = self.account_runtime.locked();
            rt.account.as_ref().map(|a| (rt.read.map_or_else(Server::current, |(s, _)| s), a.token.clone()))
        };
        let Some((server, token)) = held else { return false };
        if next.is_some_and(|(s, t)| s == server && t == token) {
            return false;
        }
        self.calls_leave(Some((Api::new(server).url(""), token)));
        true
    }

    /// [`Controller::read_account_from`] with Orbiters' check of the credential done by `check` (tests: no server).
    pub(super) fn read_account_with(
        self: &Shared,
        server: Server,
        modified: Option<SystemTime>,
        again_refused: bool,
        read: impl FnOnce() -> anyhow::Result<Option<Account>>,
        check: impl FnOnce(&str) -> anyhow::Result<Check>,
    ) {
        // The generation before the file is read: a sign-out (or a sign-in) between the read and its use makes what was
        // read stale, and it is not installed (verify-controller, C-7).
        let before = self.account_runtime.locked().generation;
        let saved = match read() {
            Ok(saved) => saved,
            // Could not be read (another tool may be writing it): whose it is is not known, and it is tried again soon. On the
            // environment it was last read for, the account installed stays with its own server (a call goes on), but is
            // nobody's for N4N0 until it is read; on any other (the environment was switched), nobody is signed in.
            Err(e) => {
                log_warn!("account file: {e:#}", e = e);
                let stays = {
                    let rt = self.account_runtime.locked();
                    rt.generation == before && rt.account.is_some() && rt.read.is_some_and(|(s, _)| s == server)
                };
                if !stays && self.account_runtime.locked().generation == before {
                    self.leave_unless_same(None);
                }
                let generation = self.install_account(|rt| {
                    (rt.generation == before).then(|| {
                        if stays {
                            rt.unknown = true;
                        } else {
                            rt.unsettled();
                        }
                        rt.unreadable = Some((server, modified, Instant::now()));
                        rt.generation
                    })
                });
                if let Some(generation) = generation {
                    self.update_if(|s| {
                        if self.account_generation.load(Ordering::SeqCst) != generation || s.account.state == AccountState::SigningIn {
                            return false;
                        }
                        if stays {
                            // Said, or "Connecting" would stay with no reason (verify5-r21 m3).
                            let shown = std::mem::replace(&mut s.account.unreadable, true);
                            s.account.error = Some(unreadable_now_text().into());
                            return !shown;
                        }
                        s.account.development = server == Server::Development;
                        s.account.state = AccountState::SignedOut;
                        s.account.error = Some(unreadable_text().into());
                        true
                    });
                }
                return;
            }
        };
        // Another account, another environment, or nobody: the account installed now leaves before the new one is put in
        // its place (and before "Checking" shows its name).
        if self.account_runtime.locked().generation == before {
            self.leave_unless_same(saved.as_ref().map(|a| (server, a.token.as_str())));
        }
        let installed = self.install_account(|rt| {
            if rt.generation != before {
                return None;
            }
            let previous_token = rt.account.as_ref().map(|a| a.token.clone());
            let refusals = if again_refused { rt.refused.map_or(0, |(_, n)| n) } else { 0 };
            rt.settled(server, modified, saved.clone());
            Some((previous_token, rt.generation, refusals))
        });
        let Some((previous_token, generation, refusals)) = installed else { return };
        let development = server == Server::Development;
        let current = || self.account_generation.load(std::sync::atomic::Ordering::SeqCst) == generation;
        let Some(saved) = saved else {
            self.update_if(|s| {
                if !current() {
                    return false;
                }
                s.account.development = development;
                if s.account.state != AccountState::SigningIn {
                    s.account.state = AccountState::SignedOut;
                }
                readable_again(s);
                true
            });
            return;
        };
        // A refused credential asked about again shows as refused until an answer says otherwise.
        if !again_refused {
            self.update_if(|s| {
                // Signed out meanwhile: "Checking" would show an account that is gone.
                if !current() {
                    return false;
                }
                readable_again(s);
                let same_server = s.account.development == development;
                s.account.development = development;
                // The same account checked again (another Orbiters tool rewrote the shared file): it stays signed in while
                // Orbiters answers, so that VRChat's sign-in, friends and invites are not switched off for the check.
                if !rechecking_same(&s.account.state, previous_token.as_deref(), &saved.token, same_server) {
                    s.account.state = AccountState::Checking { username: saved.username.clone() };
                }
                true
            });
        }
        let answer = check(&saved.token);
        // The account's features as the check says them (N4N0 calls), for once the lock is let go.
        let mut features = None;
        // Whose the saved account is, which its file (an older version's) does not say.
        let mut learned = None;
        let state = self.install_account(|rt| {
            // Signed in or out meanwhile: that newer state wins.
            if rt.generation != generation || rt.account.as_ref().map(|a| &a.token) != Some(&saved.token) {
                return None;
            }
            Some(match answer {
                Ok(Check::Valid(current)) => {
                    // For the account as it is kept just below (its id as the server says it), with this credential.
                    features = Some((current.user.clone(), current.features.clone()));
                    let name = current.username.clone().or(saved.username.clone()).unwrap_or_default();
                    learned = (saved.user.is_none() && current.user.is_some()).then(|| Account { token: saved.token.clone(), username: Some(name.clone()).filter(|n| !n.is_empty()), features: None, ..current.clone() });
                    rt.account = Some(Account { token: saved.token.clone(), ..current });
                    AccountState::SignedIn { username: name }
                }
                Ok(Check::Refused) => {
                    rt.refused = Some((Instant::now(), refusals.saturating_add(1)));
                    AccountState::Expired { username: saved.username.clone() }
                }
                Err(e) => {
                    log_warn!("account check: {e:#}", e = e);
                    rt.offline_since = Some(Instant::now());
                    AccountState::Offline { username: saved.username.clone() }
                }
            })
        });
        let Some(state) = state else { return };
        if let Some(account) = learned {
            // Tests never touch the Unity tools' account.
            if !cfg!(test) {
                self.account_learned(server, generation, &account, || account::read_saved(server), |a| account::save(server, a), || account::modified(server));
            }
        }
        if let Some((user, features)) = features {
            self.calls_access_checked(server, user.as_deref(), &saved.token, features.as_ref());
        }
        self.commit_account_check(generation, state);
        // Calls follow the account read now at once (another tool signed in as someone else).
        self.calls_account_changed();
    }

    /// Shows what a check of the saved account found, unless a sign-in, its cancel or a sign-out came since it was
    /// asked: checked under the snapshot's lock, where those publish their own state after they bumped the generation,
    /// so an old "Signed in" can no longer land after "Signed out" (C-7).
    fn commit_account_check(&self, generation: u64, state: AccountState) -> bool {
        self.update_if(|s| {
            if self.account_generation.load(std::sync::atomic::Ordering::SeqCst) != generation {
                return false;
            }
            s.account.state = state;
            s.account.error = None;
            s.account.unreadable = false;
            true
        })
    }

    /// Starts a browser sign-in: listens on this computer's loopback, asks Orbiters for a link that names where, and
    /// opens it. The person signs in on the website if needed and clicks Connect; the browser then brings the code here,
    /// which the link's secret trades for the credential ([`Controller::serve_sign_in`]). "Finish in your browser" shows
    /// at once, before Orbiters answers.
    pub fn sign_in(self: &Shared) -> anyhow::Result<()> {
        let server = Server::current();
        self.sign_in_with(server, Api::new(server))
    }

    /// [`Controller::sign_in`] through `api`, `server`'s (tests: a stand-in for Orbiters).
    fn sign_in_with(self: &Shared, server: Server, api: Api) -> anyhow::Result<()> {
        let shown = self.snapshot.locked().account.state.clone();
        let (generation, before) = {
            let mut rt = self.account_runtime.locked();
            rt.generation += 1;
            self.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            // The one waiting before goes (its listener with it); what the account showed before it comes back at the
            // end. (Another still asking Orbiters for its link shows "finishing": the account at rest then.)
            let earlier = rt.signing_in.take().map(|e| e.before.clone());
            let before = earlier.unwrap_or(if shown == AccountState::SigningIn { rt.resting() } else { shown });
            (rt.generation, before)
        };
        self.update(|s| {
            s.account.development = server == Server::Development;
            s.account.state = AccountState::SigningIn;
            s.account.error = None;
        });
        let started = (|| {
            let callback = Callback::bind()?;
            let link = api.begin_link(callback.port(), callback.path())?;
            // The links come from the API: the browser is sent only to the website's own pages.
            anyhow::ensure!(
                server.is_website_page(&link.link_url) && server.is_website_page(&link.connected_url),
                "{}",
                tr("Orbiters answered with a sign-in link that is not on its website.")
            );
            Ok((callback, link))
        })();
        let (callback, link) = match started {
            Ok(started) => started,
            Err(e) => {
                // Unless a newer sign-in or a sign-out came meanwhile, the account shows what it did before.
                let mut rt = self.account_runtime.locked();
                if rt.generation == generation {
                    self.check_restored_account(&mut rt, &before);
                    drop(rt);
                    self.update(|s| s.account.state = before);
                }
                return Err(e);
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut rt = self.account_runtime.locked();
            if rt.generation != generation {
                // Cancelled while Orbiters answered.
                return Ok(());
            }
            rt.signing_in = Some(SignIn { server, link_url: link.link_url.clone(), generation, stop: stop.clone(), before });
        }
        let ctl = self.clone();
        let url = link.link_url.clone();
        std::thread::Builder::new().name("sign-in".into()).spawn(move || ctl.serve_sign_in(&api, server, callback, link, generation, &stop))?;
        // Tests open no browser.
        if cfg!(test) {
            return Ok(());
        }
        open_url(&url)
    }

    /// The listener of a sign-in, until the browser brings a code that ends it, the sign-in is cancelled or replaced,
    /// or its link's time runs out. Each code is traded, the account saved for every tool, and only then the browser
    /// is sent on to the website's connected page (or told it failed).
    fn serve_sign_in(self: &Shared, api: &Api, server: Server, callback: Callback, link: account::Link, generation: u64, stop: &AtomicBool) {
        let served = callback.serve(stop, Instant::now() + LINK_LIFETIME, |code| {
            let exchange = api.exchange_link(&link.exchange_token, code);
            // The account signed in before leaves first, before B is installed (unless this sign-in was cancelled).
            if let Exchange::Done(account) = &exchange {
                if self.account_runtime.locked().generation == generation {
                    self.leave_unless_same(Some((server, account.token.as_str())));
                }
            }
            // Cancelled while Orbiters answered: nothing is kept.
            let cancelled = || account::Answer { location: link.failed_url(), done: true };
            let answer = link.answer(&exchange);
            if let Exchange::Done(account) = exchange {
                let name = account.username.clone().unwrap_or_default();
                let saved = self.install_account(|rt| {
                    if rt.generation != generation {
                        return None;
                    }
                    rt.signing_in = None;
                    // Tests never touch the Unity tools' account.
                    let saved = if cfg!(test) { Ok(()) } else { account::save(server, &account) };
                    rt.settled(server, account::modified(server), Some(account));
                    Some(saved)
                });
                let Some(saved) = saved else { return cancelled() };
                if let Err(e) = saved {
                    self.notify(NoticeKind::Warning, tr!("Signed in, but the Unity tools will not share it: {error}", error = e));
                }
                // Calls follow the account signed in now at once.
                self.calls_account_changed();
                // Shown only if no sign-out or cancel came since the lock above (verify-controller): under the
                // snapshot's lock, against the generation those bump before they publish.
                if self.commit_account_check(generation, AccountState::SignedIn { username: name.clone() }) {
                    self.notify(NoticeKind::Success, tr!("Signed in to Orbiters as {name}.", name = name));
                }
                return answer;
            }
            let mut rt = self.account_runtime.locked();
            if rt.generation != generation {
                return cancelled();
            }
            match exchange {
                // Installed above.
                Exchange::Done(_) => {}
                // The browser can bring another (Connect clicked again): the error shows meanwhile.
                Exchange::Retry(message) => {
                    drop(rt);
                    self.update_if(|s| {
                        let current = self.account_generation.load(std::sync::atomic::Ordering::SeqCst) == generation;
                        if current {
                            s.account.error = Some(message);
                        }
                        current
                    });
                }
                Exchange::Ended(message) => {
                    let before = rt.signing_in.take().map(|s| s.before.clone());
                    before.iter().for_each(|b| self.check_restored_account(&mut rt, b));
                    drop(rt);
                    self.update_if(|s| {
                        let current = self.account_generation.load(std::sync::atomic::Ordering::SeqCst) == generation;
                        if current {
                            s.account.state = before.unwrap_or(AccountState::SignedOut);
                            s.account.error = Some(message);
                        }
                        current
                    });
                }
            }
            answer
        });
        if served == Served::TimedOut {
            let mut rt = self.account_runtime.locked();
            if rt.signing_in.as_ref().is_some_and(|s| s.generation == generation) {
                let before = rt.signing_in.take().map(|s| s.before.clone());
                before.iter().for_each(|b| self.check_restored_account(&mut rt, b));
                drop(rt);
                self.update(|s| {
                    s.account.state = before.unwrap_or(AccountState::SignedOut);
                    s.account.error = Some(tr("The sign-in expired. Sign in again.").into());
                });
            }
        }
    }

    /// Opens the sign-in page again (closed by mistake).
    pub fn reopen_sign_in(&self) -> anyhow::Result<()> {
        let link = self.account_runtime.locked().signing_in.as_ref().map(|s| (s.server, s.link_url.clone()));
        match link {
            Some((server, url)) if server.is_website_page(&url) => open_url(&url),
            _ => Ok(()),
        }
    }

    /// Stops waiting for the browser: the listener closes, and the account shows what it did before.
    pub fn cancel_sign_in(&self) {
        let before = {
            let mut rt = self.account_runtime.locked();
            rt.generation += 1;
            self.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            let before = rt.signing_in.take().map(|s| s.before.clone()).unwrap_or_else(|| rt.resting());
            self.check_restored_account(&mut rt, &before);
            before
        };
        self.update(|s| {
            s.account.error = None;
            if s.account.state == AccountState::SigningIn {
                s.account.state = before;
            }
        });
    }

    /// Signs out of Orbiters in every tool (the saved account is removed, as in Unity).
    pub fn sign_out(&self) -> anyhow::Result<()> {
        let server = Server::current();
        self.sign_out_with(server, || {
            let _file = ACCOUNT_FILE.locked();
            account::remove(server)
        })
    }

    /// Orbiters said whose the saved account is, which its file does not (an older version wrote it): the file says it from
    /// now on, so that a restart knows it at once (verify4-r21 P8). Only for the credential the file still holds: what
    /// another tool wrote, or a sign-out, stays.
    pub(super) fn account_learned(
        &self,
        server: Server,
        generation: u64,
        account: &Account,
        read: impl Fn() -> anyhow::Result<Option<Account>>,
        save: impl FnOnce(&Account) -> anyhow::Result<()>,
        modified: impl Fn() -> Option<SystemTime>,
    ) {
        let _file = ACCOUNT_FILE.locked();
        if self.account_runtime.locked().generation != generation {
            return;
        }
        let was = modified();
        match read() {
            Ok(Some(held)) if held.token == account.token && held.user.is_none() => {}
            _ => return,
        }
        // Written by another tool since it was read: theirs stays (verify5-r21 m8).
        if modified() != was {
            return;
        }
        if let Err(e) = save(account) {
            log_warn!("account file: {e:#}", e = e);
            return;
        }
        // What the file holds is what was written here, or it is read again as the change it is.
        let written = modified();
        if !matches!(read(), Ok(Some(held)) if held.token == account.token && held.user == account.user) || modified() != written {
            return;
        }
        let mut rt = self.account_runtime.locked();
        // Not a change to read again: it is what was written here.
        if rt.generation == generation && rt.read.is_some_and(|(s, _)| s == server) {
            rt.read = Some((server, written));
        }
    }

    /// [`Controller::sign_out`] with the saved account removed by `remove` (tests: no file of the person's). Only once it
    /// is removed does the account leave: a removal that failed signs nothing out, and a call goes on.
    pub(super) fn sign_out_with(&self, server: Server, remove: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
        remove()?;
        // The account leaves before it is gone from the runtime (its credential for what Orbiters hears, PRIV-7).
        self.leave_unless_same(None);
        self.install_account(|rt| {
            rt.generation += 1;
            self.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            rt.signing_in = None;
            rt.settled(server, None, None);
        });
        self.update(|s| {
            s.account.state = AccountState::SignedOut;
            s.account.error = None;
        });
        // "My maps" was the account that left (CTL2-8).
        self.forget_maps();
        Ok(())
    }

    /// The API and credential when an account is saved; the outer `None` until the saved account was read at all.
    pub(super) fn language_account(&self) -> Option<Option<(Api, String)>> {
        let rt = self.account_runtime.locked();
        let (server, _) = rt.read?;
        Some(rt.account.as_ref().map(|a| (Api::new(server), a.token.clone())))
    }

    /// The server and the saved account's name and id, when one is saved: whom world labels go under.
    /// The server, the account's id and the API with its credential, read together (what a send goes as and the list
    /// it was taken from are the same account).
    pub(super) fn account_with_api(&self) -> Option<(Server, Option<String>, Api, String)> {
        let rt = self.account_runtime.locked();
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (server, a.user.clone(), Api::new(server), a.token.clone()))
    }

    /// Tests: an account signed in as `user` on `server`, as the account thread would have read it.
    #[cfg(test)]
    pub(super) fn signed_in_for_test(&self, server: Server, user: &str) {
        let mut rt = self.account_runtime.lock().unwrap();
        rt.read = Some((server, None));
        rt.account = Some(Account { token: format!("orbit-test-{user}"), user: Some(user.into()), username: Some(user.into()), avatar_url: None, features: None });
    }

    pub(super) fn feedback_account(&self) -> Option<(Server, Option<String>, Option<String>)> {
        let rt = self.account_runtime.locked();
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (server, a.username.clone(), a.user.clone()))
    }

    /// The account the beta's usage statistics are linked to (`usage::spool`): its id, the API address its credential is for and the
    /// credential, only while it is signed in and checked (not while it is being checked, refused or expired). An account file that
    /// names no id is known by its lower-cased name, on this computer only (the server learns the account from the credential).
    pub fn usage_account(&self) -> Option<crate::usage::spool::Signed> {
        if !matches!(self.view().account.state, AccountState::SignedIn { .. }) {
            return None;
        }
        let rt = self.account_runtime.locked();
        let (server, _) = rt.read?;
        let a = rt.account.as_ref()?;
        let account = a.user.clone().filter(|u| !u.trim().is_empty()).or_else(|| a.username.as_ref().map(|n| format!("name:{}", n.trim().to_lowercase())))?;
        Some(crate::usage::spool::Signed { account, base: Api::new(server).url(""), token: a.token.clone() })
    }

    /// Whom what the usage statistics count now is for: linked to the signed-in account only once the notice saying so was shown.
    pub fn usage_stamp(&self) -> crate::usage::spool::Stamp {
        let told = self.view().settings.usage.told_account;
        crate::usage::spool::Stamp::now(told, if told { self.usage_account().map(|s| (s.account, s.base)) } else { None })
    }

    /// The API and credential for calls to Orbiters, when an account is saved (checked or not).
    pub fn account_api(&self) -> Option<(Api, String)> {
        let rt = self.account_runtime.locked();
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (Api::new(server), a.token.clone()))
    }

    /// [`Self::account_api`] with whom it is for (read together, so the two agree): the server's API address and the
    /// account's id (a digest of its credential when the account file names no id; none when nobody is signed in).
    pub(super) fn account_api_for(&self) -> (Option<(Api, String)>, orbiters_core::world_feedback::Stamp) {
        let rt = self.account_runtime.locked();
        // The environment chosen now; the account read only when it was read for that environment (just after a switch
        // the account thread may still hold the other one's: nobody, then, rather than the other server's account).
        let server = Server::current();
        let api = Api::new(server);
        let account = rt.account.as_ref().filter(|_| rt.read.is_some_and(|(s, _)| s == server));
        let stamp = orbiters_core::world_feedback::Stamp {
            server: api.url(""),
            account: account.and_then(|a| a.user.clone()).filter(|u| !u.trim().is_empty()),
            credential: account.map(|a| format!("credential:{}", orbiters_core::vrchat_accounts::friend_hash(&a.token))),
        };
        let with = account.map(|a| (api, a.token.clone()));
        (with, stamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XCT-31's named exception: whose credential it is is never half changed. A panic with it starts it over, as before the
    /// account's file is read, and the next one read is installed as any other.
    #[test]
    fn a_panic_with_the_account_held_leaves_no_half_of_one() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        ctl.sign_in_for_test("1");
        assert!(ctl.account_api().is_some());
        let generation = ctl.account_generation.load(Ordering::SeqCst);
        let c = ctl.clone();
        let panicked = std::thread::spawn(move || {
            let mut rt = c.account_runtime.locked();
            rt.account.as_mut().unwrap().token = "orbit-test-another".into();
            panic!("between the credential and whose it is, on purpose");
        });
        assert!(panicked.join().is_err());
        assert!(ctl.account_api().is_none(), "nobody's credential goes out");
        assert_eq!(ctl.signed_as(), orbiters_n4n0::status::SignedAs::Unknown);
        assert!(!ctl.commit_account_check(generation, AccountState::SignedIn { username: "previous".into() }));
        assert!(ctl.account_runtime.locked().read.is_none(), "read again at the next look");
        ctl.install_account(|rt| rt.settled(Server::current(), None, Some(Account { token: "orbit-test-1".into(), user: Some("1".into()), username: None, avatar_url: None, features: None })));
        assert_eq!(ctl.signed_as(), orbiters_n4n0::status::SignedAs::Member);
    }
    use crate::controller::Controller;
    use orbiters_core::settings::Settings;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;

    // --- Verify2-r21 M1: N4N0's conversation belongs to the Orbiters member (environment and member id), not to the
    // credential. ---

    fn member(token: &str, user: &str) -> Account {
        Account { token: token.into(), user: Some(user.into()), username: Some(format!("member {user}")), avatar_url: None, features: None }
    }

    /// The saved account read again (another tool signed in or out, the environment switched), Orbiters' check answering
    /// `checked` (`None`: offline).
    fn reread(ctl: &Shared, server: Server, saved: Option<Account>, checked: Option<Account>) {
        ctl.read_account_with(server, None, false, move || Ok(saved), move |_| match checked {
            Some(account) => Ok(Check::Valid(account)),
            None => Err(anyhow::anyhow!("offline in tests")),
        });
    }

    /// Something said to N4N0 that it does by itself (no server): a line of the conversation.
    fn say(ctl: &Shared) {
        let stamp = orbiters_n4n0::Stamp { epoch: ctl.identity_epoch(), question: None, surface: orbiters_n4n0::Surface::Window, seq: orbiters_n4n0::next_input() };
        ctl.n4n0_command(crate::ipc::N4n0Command::Ask { text: "turn on fast walk".into(), stamp: Some(stamp) });
    }

    fn lines(ctl: &Shared) -> usize {
        ctl.snapshot.lock().unwrap().n4n0.lines.len()
    }

    /// Every transition the controller has, and whether N4N0's conversation goes with it.
    #[test]
    fn the_conversation_belongs_to_the_member_not_the_credential() {
        type Step = fn(&Shared);
        let prod = Server::Production;
        let rows: Vec<(&str, Step, Step, bool)> = vec![
            ("anonymous, then the first sign-in (the saved account read)", |_| {}, |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), true),
            ("a token refreshed for the same member", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Production, Some(member("orbit-a2", "1")), None), false),
            ("the same credential read again", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), false),
            ("Checking, then Signed in as the same member", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Production, Some(member("orbit-a", "1")), Some(member("orbit-a", "1"))), false),
            ("another member", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Production, Some(member("orbit-b", "2")), None), true),
            ("the environment switched (the same id on another server)", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Development, Some(member("orbit-d", "1")), None), true),
            ("another tool signed out (the file gone)", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| reread(c, Server::Production, None, None), true),
            ("signed out here", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| c.sign_out_with(Server::Production, || Ok(())).unwrap(), true),
            ("the file unreadable", |c| reread(c, Server::Production, Some(member("orbit-a", "1")), None), |c| c.read_account_with(Server::Production, None, false, || Err(anyhow::anyhow!("being written")), |_| Err(anyhow::anyhow!("offline"))), true),
            ("anonymous, the environment switched with nobody saved", |_| {}, |c| reread(c, Server::Development, None, None), false),
        ];
        let _ = prod;
        let mut wrong = Vec::new();
        for (name, before, change, clears) in rows {
            let ctl = Controller::new(Settings::default());
            before(&ctl);
            say(&ctl);
            let said = lines(&ctl);
            assert!(said > 0, "{name}: something was said");
            let epoch = ctl.identity_epoch();
            change(&ctl);
            let after = lines(&ctl);
            assert_eq!(ctl.identity_epoch(), epoch + clears as u64, "{name}: the epoch");
            assert!(!clears || ctl.snapshot.lock().unwrap().n4n0.account_epoch == epoch + 1, "{name}: the windows are shown the new one");
            if clears != (after == 0) {
                wrong.push(format!("{name}: {said} lines before, {after} after (expected {})", if clears { "none" } else { "kept" }));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Verify3 B1: a credential without a member id inherits nothing by its name, and neither does the id that comes after it;
    /// while it is unknown nobody's account is had for what N4N0 does.
    #[test]
    fn a_credential_without_an_id_inherits_nothing_by_its_name() {
        use orbiters_n4n0::status::SignedAs;
        let c = Controller::new(Settings::default());
        reread(&c, Server::Production, Some(member("fake-a", "1")), None);
        say(&c);
        assert!(lines(&c) > 0);
        let epoch = c.identity_epoch();
        let mut b = member("fake-b", "2");
        (b.user, b.username) = (None, Some("member 1".into()));
        reread(&c, Server::Production, Some(b.clone()), None);
        assert_eq!((lines(&c), c.identity_epoch()), (0, epoch + 1), "entering it is another identity");
        assert_eq!(c.signed_as(), SignedAs::Unknown);
        assert_eq!(c.snapshot.lock().unwrap().n4n0.signed_as, SignedAs::Unknown, "the windows are told");
        assert!(c.account_api_of(epoch + 1).is_none() && c.account_api_of(epoch).is_none());
        say(&c);
        assert!(!c.snapshot.lock().unwrap().n4n0.lines.iter().any(|l| l.text.contains("fast walk")), "nothing is done for it");
        // Read again (the same file, still without one): the same.
        reread(&c, Server::Production, Some(b.clone()), None);
        assert_eq!(c.identity_epoch(), epoch + 1);
        // Orbiters says who it is: another identity again, whoever that is (the first one's id brings their conversation back,
        // verify4 P8; another's brings nothing).
        reread(&c, Server::Production, Some(b.clone()), Some(member("fake-b", "2")));
        assert_eq!((c.identity_epoch(), c.signed_as()), (epoch + 2, SignedAs::Member));
        assert_eq!(lines(&c), 0);
        assert!(c.account_api_of(epoch + 2).is_some_and(|(_, token)| token == "fake-b"));
    }

    /// Verify3 B2: the environment switched, its account file cannot be read: signed out there, with why; the credential of
    /// the environment before is paired with nothing, and what N4N0 had of its member is gone. Readable again: signed in.
    #[test]
    fn an_unreadable_file_after_an_environment_switch_signs_out() {
        use orbiters_n4n0::status::SignedAs;
        let c = Controller::new(Settings::default());
        reread(&c, Server::Production, Some(member("fake-a", "1")), None);
        say(&c);
        let old = c.identity_epoch();
        c.account_environment_changed();
        let nothing = |what: &str| {
            for pair in [c.account_api_of(old), c.account_api_of(c.identity_epoch()), c.account_api(), c.account_api_for().0, c.language_account().flatten()] {
                assert!(pair.is_none(), "{what}: the previous credential, at {}", pair.unwrap().0.url(""));
            }
            assert!(c.account_with_api().is_none() && c.usage_account().is_none() && c.calls_identity().is_none() && c.vrchat_now().is_none(), "{what}");
            assert_eq!((lines(&c), c.signed_as()), (0, SignedAs::Unknown), "{what}");
        };
        nothing("between the two");
        let epoch = c.identity_epoch();
        assert_eq!(epoch, old + 1);
        for _ in 0..3 {
            c.read_account_with(Server::Development, None, false, || Err(anyhow::anyhow!("fixture")), |_| unreachable!("nothing is checked"));
            nothing("unreadable");
            assert_eq!(c.identity_epoch(), epoch, "still unknown: the same");
            let view = c.view();
            assert!(view.account.development && view.account.state == AccountState::SignedOut, "{:?}", view.account);
            assert_eq!(view.account.error.as_deref(), Some(unreadable_text()), "and why");
        }
        reread(&c, Server::Development, Some(member("fake-d", "1")), Some(member("fake-d", "1")));
        assert_eq!((c.identity_epoch(), c.signed_as()), (epoch + 1, SignedAs::Member));
        assert!(c.account_api_of(epoch + 1).is_some_and(|(api, token)| token == "fake-d" && api.url("") == Api::new(Server::Development).url("")));
        assert!(c.view().account.error.is_none());
        // Without a switch: it is nobody's for N4N0 until it is read, what is installed staying with its own server (a call).
        say(&c);
        let epoch = c.identity_epoch();
        c.read_account_with(Server::Development, None, false, || Err(anyhow::anyhow!("fixture")), |_| unreachable!());
        assert_eq!((c.signed_as(), c.identity_epoch(), lines(&c)), (SignedAs::Unknown, epoch + 1, 0));
        assert!(c.account_api_of(epoch).is_none() && c.account_api_of(epoch + 1).is_none());
        assert!(c.account_api().is_some_and(|(api, token)| token == "fake-d" && api.url("") == Api::new(Server::Development).url("")));
        // And readable with nobody in it: signed out, the note gone.
        reread(&c, Server::Development, None, None);
        assert_eq!((c.signed_as(), c.view().account.error.clone()), (SignedAs::Nobody, None));
    }

    /// An account being read as the environment is switched is not installed after it.
    #[test]
    fn a_read_under_way_at_a_switch_is_not_installed() {
        let c = Controller::new(Settings::default());
        c.read_account_with(Server::Production, None, false, || { c.account_environment_changed(); Ok(Some(member("fake-a", "1"))) }, |_| Err(anyhow::anyhow!("offline")));
        assert!(c.account_api().is_none() && c.account_runtime.lock().unwrap().account.is_none());
    }

    /// Verify4-r21 P8: the file being written again by another tool (nobody known meanwhile) keeps the same member's conversation, out
    /// of everyone's reach until they are known again; what N4N0 asked, and what their windows stamped before, is gone all the
    /// same. Another member, and a sign-out, drop it for good.
    #[test]
    fn the_same_members_history_comes_back_and_nobody_elses() {
        let unreadable = |c: &Shared| c.read_account_with(Server::Production, None, false, || Err(anyhow::anyhow!("being written")), |_| unreachable!());
        let said = || {
            let c = Controller::new(Settings::default());
            reread(&c, Server::Production, Some(member("fake-a", "1")), None);
            say(&c);
            assert!(lines(&c) > 0);
            c
        };
        let c = said();
        let epoch = c.identity_epoch();
        unreadable(&c);
        assert_eq!((lines(&c), c.identity_epoch()), (0, epoch + 1));
        unreadable(&c);
        // Their own credential again, refreshed, of no known member until Orbiters says.
        let mut bare = member("fake-a2", "1");
        bare.user = None;
        reread(&c, Server::Production, Some(bare.clone()), None);
        assert_eq!(lines(&c), 0);
        reread(&c, Server::Production, Some(bare), Some(member("fake-a2", "1")));
        assert!(lines(&c) > 0, "the same member's conversation is gone");
        assert!(c.identity_epoch() > epoch + 1, "what was stamped before is still nothing");
        // Someone else comes first, or a sign-out, or the same id of another environment.
        type Step = fn(&Shared);
        let between: [(&str, Step); 3] = [
            ("another member", |c| reread(c, Server::Production, Some(member("fake-b", "2")), None)),
            ("a sign-out", |c| reread(c, Server::Production, None, None)),
            ("another environment", |c| reread(c, Server::Development, Some(member("fake-d", "1")), None)),
        ];
        for (what, step) in between {
            let c = said();
            unreadable(&c);
            step(&c);
            assert_eq!(lines(&c), 0, "{what}");
            reread(&c, Server::Production, Some(member("fake-a", "1")), None);
            assert_eq!(lines(&c), 0, "{what}, then the first one again");
        }
    }

    /// Verify5-r21 m3: a file that was read and cannot be read any more is said, by the account and by what waits for it, until it
    /// is read again.
    #[test]
    fn an_account_that_cannot_be_read_any_more_says_so() {
        let c = Controller::new(Settings::default());
        reread(&c, Server::Production, Some(member("fake-a", "1")), Some(member("fake-a", "1")));
        assert!(!c.view().account.unreadable && c.view().account.error.is_none());
        for _ in 0..2 {
            c.read_account_with(Server::Production, None, false, || Err(anyhow::anyhow!("being written")), |_| unreachable!());
            let account = c.view().account.clone();
            assert!(account.unreadable && matches!(account.state, AccountState::SignedIn { .. }), "{account:?}");
            assert_eq!(account.error.as_deref(), Some(unreadable_now_text()));
            assert!(account.connecting().contains("can't be read"), "{}", account.connecting());
        }
        reread(&c, Server::Production, Some(member("fake-a", "1")), Some(member("fake-a", "1")));
        let account = c.view().account.clone();
        assert!(!account.unreadable && account.error.is_none() && account.connecting().contains("Connecting"), "{account:?}");
    }

    /// Verify5-r21 m7: Sign in pressed while the account was being checked, then cancelled: the check was dropped with it, and the
    /// account is read and checked again.
    #[test]
    fn a_cancelled_sign_in_checks_the_account_again() {
        let c = Controller::new(Settings::default());
        let mut bare = member("fake-a", "1");
        bare.user = None;
        // The check under way as the sign-in starts.
        let (again, inner) = (c.clone(), bare.clone());
        c.read_account_with(Server::Production, Some(SystemTime::UNIX_EPOCH + Duration::from_secs(9)), false, move || Ok(Some(inner)), move |_| {
            {
                let mut rt = again.account_runtime.lock().unwrap();
                rt.generation += 1;
                again.account_generation.store(rt.generation, Ordering::SeqCst);
                let before = again.snapshot.lock().unwrap().account.state.clone();
                rt.signing_in = Some(SignIn { server: Server::Production, link_url: String::new(), generation: rt.generation, stop: Arc::new(AtomicBool::new(false)), before });
            }
            again.update(|s| s.account.state = AccountState::SigningIn);
            Ok(Check::Valid(member("fake-a", "1")))
        });
        assert_eq!(c.view().account.state, AccountState::SigningIn);
        let modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(9));
        let waiting = c.rest("account", |_| ());
        assert!(!waiting.wait(Some(Duration::ZERO)));
        c.cancel_sign_in();
        assert!(waiting.wait(Some(Duration::ZERO)), "reread even without a file change");
        assert!(matches!(c.view().account.state, AccountState::Checking { .. }), "{:?}", c.view().account.state);
        assert!(c.account_runtime.lock().unwrap().read != Some((Server::Production, modified)), "read again at the next look");
        c.read_account_with(Server::Production, modified, false, move || Ok(Some(bare)), |_| Ok(Check::Valid(member("fake-a", "1"))));
        assert!(matches!(c.view().account.state, AccountState::SignedIn { .. }));
        assert_eq!(c.signed_as(), orbiters_n4n0::status::SignedAs::Member);
    }

    /// P8: an older file without the member's id says it once Orbiters said whose it is, so that a restart knows it at once; never over
    /// what another tool wrote since, nor after a sign-out.
    #[test]
    fn the_saved_account_learns_its_members_id() {
        let wrote = std::cell::RefCell::new(Vec::new());
        let learned = |c: &Shared, held: Option<Account>, generation: u64| {
            wrote.borrow_mut().clear();
            c.account_learned(
                Server::Production,
                generation,
                &member("fake-a", "1"),
                || Ok(wrote.borrow().last().cloned().or(held.clone())),
                |a| { wrote.borrow_mut().push(a.clone()); Ok(()) },
                || Some(SystemTime::UNIX_EPOCH + Duration::from_secs(wrote.borrow().len() as u64)),
            );
            let n = wrote.borrow().len();
            n
        };
        let c = Controller::new(Settings::default());
        let mut bare = member("fake-a", "1");
        bare.user = None;
        reread(&c, Server::Production, Some(bare.clone()), Some(member("fake-a", "1")));
        let generation = c.account_runtime.lock().unwrap().generation;
        // Written by another tool between the read and the write: theirs stays. Just after the write: read again as a change.
        let at = |n: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(n));
        let before = c.account_runtime.lock().unwrap().read;
        for theirs_after in [false, true] {
            let (clock, saved) = (std::cell::Cell::new(0u64), std::cell::Cell::new(false));
            c.account_learned(
                Server::Production,
                generation,
                &member("fake-a", "1"),
                || Ok(Some(if saved.get() { Account { token: "fake-b".into(), ..member("fake-b", "2") } } else { bare.clone() })),
                |_| { saved.set(true); Ok(()) },
                || { if !theirs_after { clock.set(clock.get() + 1); } at(clock.get()) },
            );
            assert_eq!(saved.get(), theirs_after);
            assert_eq!(c.account_runtime.lock().unwrap().read, before, "not taken for what was written here");
        }
        assert_eq!(learned(&c, Some(bare.clone()), generation), 1);
        assert_eq!(wrote.borrow()[0].user.as_deref(), Some("1"));
        assert_eq!(c.account_runtime.lock().unwrap().read, Some((Server::Production, at(1))), "not a change to read again");
        for (what, held) in [("already there", Some(member("fake-a", "1"))), ("signed out elsewhere", None), ("another credential", Some(Account { token: "fake-b".into(), ..bare.clone() }))] {
            assert_eq!(learned(&c, held, generation), 0, "{what}");
        }
        c.sign_out_with(Server::Production, || Ok(())).unwrap();
        assert_eq!(learned(&c, Some(bare), generation), 0, "signed out here");
    }

    /// A refused credential is asked about again, later each time, never more often than every ten minutes.
    #[test]
    fn a_refused_credential_is_asked_about_again_later_each_time() {
        let waits: Vec<u64> = (1..=8).map(|n| refused_recheck(n).as_secs() / 60).collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 320, 360, 360]);
        assert_eq!(refused_recheck(0), RECHECK_REFUSED);
        assert_eq!(refused_recheck(u32::MAX), RECHECK_REFUSED_MAX);
    }

    /// The usage statistics are linked to the account only once the notice saying so was closed, and only while it is signed in and
    /// checked: before, and while checking or expired, nothing names it (2026-11-19).
    #[test]
    fn the_usage_statistics_name_the_account_only_after_the_new_notice() {
        let ctl = Controller::new(Settings::default());
        assert!(ctl.usage_stamp().is_none(), "nobody, not told");
        ctl.account_runtime.lock().unwrap().read = Some((Server::current(), None));
        ctl.account_runtime.lock().unwrap().account = Some(Account { token: "orbit-secret".into(), user: Some("42".into()), username: Some("Enzo".into()), avatar_url: None, features: None });
        ctl.update(|s| s.account.state = AccountState::SignedIn { username: "Enzo".into() });
        assert_eq!(ctl.usage_account().map(|s| (s.account, s.token)), Some(("42".into(), "orbit-secret".into())));
        assert!(ctl.usage_stamp().is_none(), "signed in, but the old notice only: nothing is linked to the account");
        ctl.update(|s| s.settings.usage.told_account = true);
        assert_eq!(ctl.usage_stamp(), crate::usage::spool::Stamp { told: true, account: Some("42".into()), server: Some(Api::new(Server::current()).url("")) });
        for state in [AccountState::Checking { username: Some("Enzo".into()) }, AccountState::Expired { username: Some("Enzo".into()) }, AccountState::SignedOut] {
            ctl.update(|s| s.account.state = state.clone());
            assert_eq!(ctl.usage_stamp(), crate::usage::spool::Stamp { told: true, account: None, server: None }, "{state:?}");
            assert!(ctl.usage_account().is_none());
        }
        // An account file without an id: known here by its name, never by its credential.
        ctl.update(|s| s.account.state = AccountState::SignedIn { username: "Enzo".into() });
        ctl.account_runtime.lock().unwrap().account = Some(Account { token: "orbit-secret".into(), user: None, username: Some(" Enzo ".into()), avatar_url: None, features: None });
        assert_eq!(ctl.usage_stamp().account.as_deref(), Some("name:enzo"));
    }

    /// verify-controller (C-7's earlier window): an account read just before a sign-out and used after it is not installed:
    /// the runtime keeps no account (no credential for calls), and neither "Checking" nor "Signed in" is shown.
    #[test]
    fn an_account_read_before_a_sign_out_is_not_installed_after_it() {
        let ctl = Controller::new(Settings::default());
        ctl.update(|s| s.account.state = AccountState::SignedIn { username: "old".into() });
        let old = Account { token: "old-token".into(), user: Some("7".into()), username: Some("old".into()), avatar_url: None, features: None };
        ctl.read_account_from(Server::Production, None, false, || {
            // Signed out while the file was being read: what `sign_out` does to the runtime and the snapshot.
            {
                let mut rt = ctl.account_runtime.lock().unwrap();
                rt.generation += 1;
                ctl.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
                rt.account = None;
            }
            ctl.update(|s| s.account.state = AccountState::SignedOut);
            Ok(Some(old))
        });
        assert!(ctl.account_runtime.lock().unwrap().account.is_none(), "no credential of the account that left");
        assert_eq!(ctl.view().account.state, AccountState::SignedOut);
    }

    /// C-7: a check answered before a sign-out and shown after it does not put "Signed in" back.
    #[test]
    fn an_old_account_check_does_not_undo_a_sign_out() {
        let ctl = Controller::new(Settings::default());
        let generation = ctl.account_runtime.lock().unwrap().generation;
        assert!(ctl.commit_account_check(generation, AccountState::Checking { username: None }), "a current check shows");
        // Signed out between the answer and its showing: what `sign_out` does to the runtime and the snapshot (not
        // `sign_out` itself, which removes the saved account file of the person running the tests).
        {
            let mut rt = ctl.account_runtime.lock().unwrap();
            rt.generation += 1;
            ctl.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            rt.account = None;
        }
        ctl.update(|s| s.account.state = AccountState::SignedOut);
        assert!(!ctl.commit_account_check(generation, AccountState::SignedIn { username: "old".into() }));
        assert_eq!(ctl.view().account.state, AccountState::SignedOut);
    }

    /// The port and path of the app's listener, as the stand-in was told them.
    type Told = Arc<Mutex<Option<(u16, String)>>>;

    /// A stand-in for Orbiters as the development server (its website on this computer): the link it hands out, the
    /// port and path of the app's listener it was told, and trades of the codes "good" (an account), "gone" (the link
    /// ended) and anything else (no match).
    fn orbiters() -> (Api, Told) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let told: Told = Arc::default();
        let (t, s) = (told.clone(), site.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, &s, &t);
            }
        });
        (Api::at(&site), told)
    }

    fn answer(mut stream: TcpStream, site: &str, told: &Mutex<Option<(u16, String)>>) {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let (request, body) = loop {
            let Ok(n) = stream.read(&mut buf) else { return };
            if n == 0 {
                return;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data).to_string();
            let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
            let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if body.len() >= length.unwrap_or(0) {
                break (head.lines().next().unwrap_or_default().to_string(), body.to_string());
            }
        };
        let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let (status, reply) = if request.starts_with("POST /editor-links/exchange ") {
            match json["code"].as_str() {
                Some("good") => (200, serde_json::json!({ "token": "orbit-test", "user": "7", "username": "Tester", "avatarUrl": null })),
                Some("gone") => (410, serde_json::json!({ "error": "This sign-in ended. Click Sign in again." })),
                _ => (400, serde_json::json!({ "error": "This sign-in code does not match." })),
            }
        } else {
            *told.lock().unwrap() = Some((json["port"].as_u64().unwrap_or(0) as u16, json["path"].as_str().unwrap_or_default().to_string()));
            let link = serde_json::json!({ "linkUrl": format!("{site}/link-editor?token=t&app=desktop"),
                "connectedUrl": format!("{site}/link-editor/connected?client=desktop"), "exchangeToken": "x", "expiresAt": "2026-10-25T12:00:00Z" });
            (200, link)
        };
        let body = reply.to_string();
        let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }

    /// The browser bringing `code` to the app's listener: the status, and where it is sent.
    fn bring(at: &(u16, String), code: &str) -> (u16, Option<String>) {
        let mut s = TcpStream::connect(("127.0.0.1", at.0)).unwrap();
        write!(s, "GET /{}?code={code} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", at.1, at.0).unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        (reply.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), reply.lines().find_map(|l| l.strip_prefix("Location: ")).map(str::to_string))
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let limit = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < limit, "waited for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn signs_in_with_the_websites_connect_and_nothing_typed() {
        let (api, told) = orbiters();
        let ctl = Controller::new(Settings::default());
        ctl.update(|s| s.account.state = AccountState::SignedOut);
        ctl.sign_in_with(Server::Development, api).unwrap();
        assert_eq!(ctl.view().account.state, AccountState::SigningIn);
        let at = told.lock().unwrap().clone().expect("the listener was named");
        // A code that does not match: the browser hears it failed, the sign-in still waits (the error shows).
        let (status, location) = bring(&at, "wrong");
        assert!(status == 302 && location.as_deref().is_some_and(|l| l.ends_with("client=desktop&failed=1")), "{location:?}");
        until("the error", || ctl.view().account.error.is_some());
        assert_eq!(ctl.view().account.state, AccountState::SigningIn);
        // Connect's code: signed in, then the browser goes to the connected page; the listener is gone.
        let (status, location) = bring(&at, "good");
        assert!(status == 302 && location.as_deref().is_some_and(|l| l.ends_with("connected?client=desktop")), "{location:?}");
        assert_eq!(ctl.view().account.state, AccountState::SignedIn { username: "Tester".into() });
        assert_eq!(ctl.view().account.error, None);
        assert!(ctl.account_api().is_some_and(|(_, token)| token == "orbit-test"));
        assert!(ctl.take_notices().iter().any(|n| format!("{n:?}").contains("Signed in to Orbiters as Tester")));
        until("the listener to close", || TcpStream::connect(("127.0.0.1", at.0)).is_err());
    }

    #[test]
    fn a_cancelled_or_ended_sign_in_puts_the_account_back() {
        let (api, told) = orbiters();
        let ctl = Controller::new(Settings::default());
        let expired = AccountState::Expired { username: Some("Old".into()) };
        ctl.update(|s| s.account.state = expired.clone());
        // Cancelled: the listener closes, the account shows what it did before.
        ctl.sign_in_with(Server::Development, api.clone()).unwrap();
        let first = told.lock().unwrap().clone().unwrap();
        ctl.cancel_sign_in();
        assert_eq!(ctl.view().account.state, expired);
        until("the listener to close", || TcpStream::connect(("127.0.0.1", first.0)).is_err());
        // A second sign-in replaces a first still waiting (the first's listener goes).
        ctl.sign_in_with(Server::Development, api.clone()).unwrap();
        let second = told.lock().unwrap().clone().unwrap();
        ctl.sign_in_with(Server::Development, api).unwrap();
        let third = told.lock().unwrap().clone().unwrap();
        until("the replaced listener to close", || TcpStream::connect(("127.0.0.1", second.0)).is_err());
        // Ended by Orbiters: the browser hears it failed, the account shows what it did before, with why.
        let (status, location) = bring(&third, "gone");
        assert!(status == 302 && location.as_deref().is_some_and(|l| l.ends_with("&failed=1")), "{location:?}");
        assert_eq!(ctl.view().account.state, expired);
        assert!(ctl.view().account.error.as_deref().is_some_and(|e| e.contains("ended")));
        until("the listener to close", || TcpStream::connect(("127.0.0.1", third.0)).is_err());
        // Orbiters out of reach: the sign-in does not start, and the account is as it was.
        assert!(ctl.sign_in_with(Server::Development, Api::at("http://127.0.0.1:9/")).is_err());
        assert_eq!(ctl.view().account.state, expired);
    }

    #[test]
    fn only_web_pages_are_opened() {
        for other in
            ["file:///C:/Windows/System32/calc.exe", "C:/Windows/System32/calc.exe", "//host/share/x.exe", "ms-msdt:/id PCWDiagnostic", "search-ms:query=x", ""]
        {
            assert!(open_url(other).is_err(), "{other}");
        }
    }
}

#[cfg(test)]
impl Controller {
    pub(super) fn test_account(&self) {
        self.account_runtime.lock().unwrap().read = Some((Server::current(), None));
        self.account_runtime.lock().unwrap().account = Some(Account { token: "fake".into(), user: Some("1".into()), username: Some("tester".into()), avatar_url: None, features: None });
        self.update(|s| { s.account.state = AccountState::SignedIn { username: "tester".into() }; s.account.development = Server::current() == Server::Development; });
    }

    /// [`Self::test_account`] with the account's numeric id.
    pub(super) fn test_account_with_user(&self, user: &str) {
        self.test_account();
        if let Some(account) = self.account_runtime.lock().unwrap().account.as_mut() {
            account.user = Some(user.into());
        }
    }
}

/// Whether a read of the saved account is a re-check of the account already signed in on show (same token, same
/// server): no "Checking" then. Another token, or any other state, is checked as before.
fn rechecking_same(shown: &AccountState, previous_token: Option<&str>, saved_token: &str, same_server: bool) -> bool {
    same_server && matches!(shown, AccountState::SignedIn { .. }) && previous_token == Some(saved_token)
}

#[cfg(test)]
mod recheck_tests {
    use super::*;

    #[test]
    fn a_recheck_of_the_same_account_keeps_it_signed_in() {
        let signed_in = AccountState::SignedIn { username: "a".into() };
        assert!(rechecking_same(&signed_in, Some("t"), "t", true));
        assert!(!rechecking_same(&signed_in, Some("t"), "other", true), "another token is checked (VRChat suspended)");
        assert!(!rechecking_same(&signed_in, Some("t"), "t", false), "another server too");
        assert!(!rechecking_same(&signed_in, None, "t", true));
        assert!(!rechecking_same(&AccountState::Expired { username: None }, Some("t"), "t", true));
    }
}
