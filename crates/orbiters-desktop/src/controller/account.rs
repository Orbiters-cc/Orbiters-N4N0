//! The Orbiters account in the tray service. It is the one the Unity tools keep (`orbiters_core::account`): read at
//! start, checked with Orbiters, and followed while the app runs, so signing in or out in MCB or My Avatar does the same
//! here within seconds. Signing in runs the tools' browser pairing: a listener on this computer's loopback waits for the
//! browser to bring the code of the website's Connect, and trades it for the credential. The credential stays in this
//! process (the window only sees the account's name and state).

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
    signing_in: Option<SignIn>,
    /// Bumped by every sign-in, cancel and sign-out, so an answer for an older attempt is dropped.
    generation: u64,
    offline_since: Option<Instant>,
    /// The credential was refused: when, and how many times in a row.
    refused: Option<(Instant, u32)>,
}

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

/// Opens a web page (http or https only) in the default browser.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let lower = url.to_ascii_lowercase();
    anyhow::ensure!(lower.starts_with("https://") || lower.starts_with("http://"), "{}", tr!("Not a web page: {url}", url = url));
    #[cfg(windows)]
    open_with_browser(url)?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
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

impl Controller {
    /// Reads the account now, then follows it (and a sign-in in progress) on a thread of its own.
    pub(super) fn start_account(self: &Shared) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("account".into()).spawn(move || loop {
            ctl.account_tick();
            std::thread::sleep(Duration::from_millis(500));
        });
    }

    fn account_tick(self: &Shared) {
        let server = Server::current();
        let modified = account::modified(server);
        let (changed, offline_due, refused_due) = {
            let rt = self.account_runtime.lock().unwrap();
            (
                rt.read != Some((server, modified)),
                rt.offline_since.is_some_and(|t| t.elapsed() > RECHECK_OFFLINE),
                rt.refused.is_some_and(|(t, n)| t.elapsed() > refused_recheck(n)),
            )
        };
        if changed || offline_due || refused_due {
            self.read_account(server, modified, !changed && refused_due);
        }
    }

    /// Reads the saved account for `server` and asks Orbiters whether it still works. `again_refused`: asking again
    /// about a credential that was refused, which shows as refused until an answer says otherwise.
    fn read_account(self: &Shared, server: Server, modified: Option<SystemTime>, again_refused: bool) {
        self.read_account_from(server, modified, again_refused, || account::read(server));
    }

    /// [`Controller::read_account`] with the saved account read by `read` (tests: no file of the person's).
    fn read_account_from(self: &Shared, server: Server, modified: Option<SystemTime>, again_refused: bool, read: impl FnOnce() -> Option<Account>) {
        // The generation before the file is read: a sign-out (or a sign-in) between the read and its use makes what was
        // read stale, and it is not installed (verify-controller, C-7).
        let before = self.account_runtime.lock().unwrap().generation;
        let saved = read();
        let previous_token;
        let (generation, refusals) = {
            let mut rt = self.account_runtime.lock().unwrap();
            if rt.generation != before {
                return;
            }
            previous_token = rt.account.as_ref().map(|a| a.token.clone());
            rt.read = Some((server, modified));
            rt.account = saved.clone();
            rt.offline_since = None;
            let refusals = if again_refused { rt.refused.map_or(0, |(_, n)| n) } else { 0 };
            rt.refused = None;
            (rt.generation, refusals)
        };
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
        let answer = Api::new(server).check(&saved.token);
        let mut rt = self.account_runtime.lock().unwrap();
        // Signed in or out meanwhile: that newer state wins.
        if rt.generation != generation || rt.account.as_ref().map(|a| &a.token) != Some(&saved.token) {
            return;
        }
        // The account's features as the check says them (N4N0 calls), for once the lock is let go.
        let mut features = None;
        let state = match answer {
            Ok(Check::Valid(current)) => {
                // For the account as it is kept just below (its id as the server says it), with this credential.
                features = Some((current.user.clone(), current.features.clone()));
                let name = current.username.clone().or(saved.username.clone()).unwrap_or_default();
                rt.account = Some(Account { token: saved.token.clone(), ..current });
                AccountState::SignedIn { username: name }
            }
            Ok(Check::Refused) => {
                rt.refused = Some((Instant::now(), refusals.saturating_add(1)));
                AccountState::Expired { username: saved.username.clone() }
            }
            Err(e) => {
                eprintln!("account check: {e:#}");
                rt.offline_since = Some(Instant::now());
                AccountState::Offline { username: saved.username.clone() }
            }
        };
        drop(rt);
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
        let shown = self.snapshot.lock().unwrap().account.state.clone();
        let (generation, before) = {
            let mut rt = self.account_runtime.lock().unwrap();
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
                if self.account_runtime.lock().unwrap().generation == generation {
                    self.update(|s| s.account.state = before);
                }
                return Err(e);
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut rt = self.account_runtime.lock().unwrap();
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
            let mut rt = self.account_runtime.lock().unwrap();
            if rt.generation != generation {
                // Cancelled while Orbiters answered: nothing is kept.
                return account::Answer { location: link.failed_url(), done: true };
            }
            let answer = link.answer(&exchange);
            match exchange {
                Exchange::Done(account) => {
                    rt.signing_in = None;
                    // Tests never touch the Unity tools' account.
                    let saved = if cfg!(test) { Ok(()) } else { account::save(server, &account) };
                    let name = account.username.clone().unwrap_or_default();
                    rt.account = Some(account);
                    rt.read = Some((server, account::modified(server)));
                    rt.offline_since = None;
                    rt.refused = None;
                    drop(rt);
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
                }
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
            let mut rt = self.account_runtime.lock().unwrap();
            if rt.signing_in.as_ref().is_some_and(|s| s.generation == generation) {
                let before = rt.signing_in.take().map(|s| s.before.clone());
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
        let link = self.account_runtime.lock().unwrap().signing_in.as_ref().map(|s| (s.server, s.link_url.clone()));
        match link {
            Some((server, url)) if server.is_website_page(&url) => open_url(&url),
            _ => Ok(()),
        }
    }

    /// Stops waiting for the browser: the listener closes, and the account shows what it did before.
    pub fn cancel_sign_in(&self) {
        let before = {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.generation += 1;
            self.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            rt.signing_in.take().map(|s| s.before.clone()).unwrap_or_else(|| rt.resting())
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
        // Calls leave Orbiters with the account that leaves, before it is gone (PRIV-7).
        self.calls_depart();
        account::remove(server)?;
        {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.generation += 1;
            self.account_generation.store(rt.generation, std::sync::atomic::Ordering::SeqCst);
            rt.signing_in = None;
            rt.account = None;
            rt.read = Some((server, None));
        }
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
        let rt = self.account_runtime.lock().unwrap();
        let (server, _) = rt.read?;
        Some(rt.account.as_ref().map(|a| (Api::new(server), a.token.clone())))
    }

    /// The server and the saved account's name and id, when one is saved: whom world labels go under.
    /// The server, the account's id and the API with its credential, read together (what a send goes as and the list
    /// it was taken from are the same account).
    pub(super) fn account_with_api(&self) -> Option<(Server, Option<String>, Api, String)> {
        let rt = self.account_runtime.lock().unwrap();
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
        let rt = self.account_runtime.lock().unwrap();
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
        let rt = self.account_runtime.lock().unwrap();
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
        let rt = self.account_runtime.lock().unwrap();
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (Api::new(server), a.token.clone()))
    }

    /// [`Self::account_api`] with whom it is for (read together, so the two agree): the server's API address and the
    /// account's id (a digest of its credential when the account file names no id; none when nobody is signed in).
    pub(super) fn account_api_for(&self) -> (Option<(Api, String)>, orbiters_core::world_feedback::Stamp) {
        let rt = self.account_runtime.lock().unwrap();
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
    use crate::controller::Controller;
    use orbiters_core::settings::Settings;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;

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
            Some(old)
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
        self.account_runtime.lock().unwrap().account = Some(Account { token: "fake".into(), user: None, username: Some("tester".into()), avatar_url: None, features: None });
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
