//! The Orbiters account in the tray service. It is the one the Unity tools keep (`orbiters_core::account`): read at
//! start, checked with Orbiters, and followed while the app runs, so signing in or out in MCB or My Avatar does the same
//! here within seconds. Signing in runs the tools' browser pairing; the credential stays in this process (the window
//! only sees the account's name and state).

use super::{Controller, NoticeKind, Shared};
use crate::state::AccountState;
use orbiters_core::account::{self, Account, Api, Check, Poll, Server};
use std::time::{Duration, Instant, SystemTime};

/// How long a browser sign-in waits for the website (the server's own link lifetime).
const LINK_LIFETIME: Duration = Duration::from_secs(600);
const POLL_EVERY: Duration = Duration::from_secs(2);
/// An account that could not be checked (offline) is checked again this often.
const RECHECK_OFFLINE: Duration = Duration::from_secs(60);

/// What the service knows besides the snapshot.
#[derive(Default)]
pub struct AccountRuntime {
    /// The signed-in account (with its credential), while one is saved for the current server.
    account: Option<Account>,
    /// The server and the account file's change time last read, to notice another tool signing in or out.
    read: Option<(Server, Option<SystemTime>)>,
    signing_in: Option<SignIn>,
    /// Bumped by every sign-in, cancel and sign-out, so an answer for an older attempt is dropped.
    generation: u64,
    offline_since: Option<Instant>,
}

struct SignIn {
    server: Server,
    poll_token: String,
    link_url: String,
    until: Instant,
    generation: u64,
    polled: Option<Instant>,
}

/// Opens a web page (http or https only) in the default browser.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let lower = url.to_ascii_lowercase();
    anyhow::ensure!(lower.starts_with("https://") || lower.starts_with("http://"), "Not a web page: {url}");
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
        anyhow::ensure!(result.0 as usize > 32, "Could not open the browser (error {}).", result.0 as usize);
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
        self.poll_sign_in();
        let server = Server::current();
        let modified = account::modified(server);
        let (changed, offline_due) = {
            let rt = self.account_runtime.lock().unwrap();
            (rt.read != Some((server, modified)), rt.offline_since.is_some_and(|t| t.elapsed() > RECHECK_OFFLINE))
        };
        if changed || offline_due {
            self.read_account(server, modified);
        }
    }

    /// Reads the saved account for `server` and asks Orbiters whether it still works.
    fn read_account(self: &Shared, server: Server, modified: Option<SystemTime>) {
        let saved = account::read(server);
        let generation = {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.read = Some((server, modified));
            rt.account = saved.clone();
            rt.offline_since = None;
            rt.generation
        };
        let development = server == Server::Development;
        let Some(saved) = saved else {
            self.update(|s| {
                s.account.development = development;
                if !matches!(s.account.state, AccountState::SigningIn { .. }) {
                    s.account.state = AccountState::SignedOut;
                }
            });
            return;
        };
        self.update(|s| {
            s.account.development = development;
            s.account.state = AccountState::Checking { username: saved.username.clone() };
        });
        let answer = Api::new(server).check(&saved.token);
        let mut rt = self.account_runtime.lock().unwrap();
        // Signed in or out meanwhile: that newer state wins.
        if rt.generation != generation || rt.account.as_ref().map(|a| &a.token) != Some(&saved.token) {
            return;
        }
        let state = match answer {
            Ok(Check::Valid(current)) => {
                let name = current.username.clone().or(saved.username.clone()).unwrap_or_default();
                rt.account = Some(Account { token: saved.token.clone(), ..current });
                AccountState::SignedIn { username: name }
            }
            Ok(Check::Refused) => AccountState::Expired { username: saved.username.clone() },
            Err(e) => {
                eprintln!("account check: {e:#}");
                rt.offline_since = Some(Instant::now());
                AccountState::Offline { username: saved.username.clone() }
            }
        };
        drop(rt);
        self.update(|s| {
            s.account.state = state;
            s.account.error = None;
        });
    }

    /// Starts a browser sign-in with `provider` (`discord` or `telegram`): opens the website and shows its code.
    pub fn sign_in(self: &Shared, provider: &str) -> anyhow::Result<()> {
        let server = Server::current();
        let link = Api::new(server).begin_link(provider)?;
        // The link comes from the API: open it only when it is the website's own page.
        anyhow::ensure!(server.is_website_page(&link.link_url), "Orbiters answered with a sign-in link that is not on its website.");
        let generation = {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.generation += 1;
            rt.signing_in = Some(SignIn {
                server,
                poll_token: link.poll_token.clone(),
                link_url: link.link_url.clone(),
                until: Instant::now() + LINK_LIFETIME,
                generation: rt.generation,
                polled: None,
            });
            rt.generation
        };
        let _ = generation;
        self.update(|s| {
            s.account.development = server == Server::Development;
            s.account.state = AccountState::SigningIn { code: link.code.clone(), provider: provider.to_string() };
            s.account.error = None;
        });
        open_url(&link.link_url)
    }

    /// Opens the sign-in page again (closed by mistake).
    pub fn reopen_sign_in(&self) -> anyhow::Result<()> {
        let link = self.account_runtime.lock().unwrap().signing_in.as_ref().map(|s| (s.server, s.link_url.clone()));
        match link {
            Some((server, url)) if server.is_website_page(&url) => open_url(&url),
            _ => Ok(()),
        }
    }

    pub fn cancel_sign_in(&self) {
        let account = {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.generation += 1;
            rt.signing_in = None;
            rt.account.clone()
        };
        self.update(|s| {
            s.account.error = None;
            s.account.state = match account {
                Some(a) => AccountState::SignedIn { username: a.username.unwrap_or_default() },
                None => AccountState::SignedOut,
            };
        });
    }

    /// Signs out of Orbiters in every tool (the saved account is removed, as in Unity).
    pub fn sign_out(&self) -> anyhow::Result<()> {
        let server = Server::current();
        account::remove(server)?;
        {
            let mut rt = self.account_runtime.lock().unwrap();
            rt.generation += 1;
            rt.signing_in = None;
            rt.account = None;
            rt.read = Some((server, None));
        }
        self.update(|s| {
            s.account.state = AccountState::SignedOut;
            s.account.error = None;
        });
        Ok(())
    }

    /// The API and credential for calls to Orbiters, when an account is saved (checked or not).
    pub fn account_api(&self) -> Option<(Api, String)> {
        let rt = self.account_runtime.lock().unwrap();
        let (server, _) = rt.read?;
        rt.account.as_ref().map(|a| (Api::new(server), a.token.clone()))
    }

    /// One poll of the browser sign-in in progress, every two seconds.
    fn poll_sign_in(self: &Shared) {
        let (server, poll_token, generation) = {
            let mut rt = self.account_runtime.lock().unwrap();
            let Some(attempt) = rt.signing_in.as_mut() else { return };
            if Instant::now() > attempt.until {
                rt.signing_in = None;
                let had = rt.account.clone();
                drop(rt);
                self.update(|s| {
                    s.account.state = match had {
                        Some(a) => AccountState::SignedIn { username: a.username.unwrap_or_default() },
                        None => AccountState::SignedOut,
                    };
                    s.account.error = Some("The sign-in expired. Sign in again.".into());
                });
                return;
            }
            if attempt.polled.is_some_and(|t| t.elapsed() < POLL_EVERY) {
                return;
            }
            attempt.polled = Some(Instant::now());
            (attempt.server, attempt.poll_token.clone(), attempt.generation)
        };
        let answer = Api::new(server).poll_link(&poll_token);
        let mut rt = self.account_runtime.lock().unwrap();
        if rt.signing_in.as_ref().map(|s| s.generation) != Some(generation) {
            return;
        }
        match answer {
            Ok(Poll::Pending) => {}
            Ok(Poll::Done(account)) => {
                rt.signing_in = None;
                let saved = account::save(server, &account);
                let name = account.username.clone().unwrap_or_default();
                rt.account = Some(account);
                rt.read = Some((server, account::modified(server)));
                rt.offline_since = None;
                drop(rt);
                if let Err(e) = saved {
                    self.notify(NoticeKind::Warning, format!("Signed in, but the Unity tools will not share it: {e}"));
                }
                self.update(|s| {
                    s.account.state = AccountState::SignedIn { username: name.clone() };
                    s.account.error = None;
                });
                self.notify(NoticeKind::Success, format!("Signed in to Orbiters as {name}."));
            }
            Err(e) => {
                rt.signing_in = None;
                let had = rt.account.clone();
                drop(rt);
                self.update(|s| {
                    s.account.state = match had {
                        Some(a) => AccountState::SignedIn { username: a.username.unwrap_or_default() },
                        None => AccountState::SignedOut,
                    };
                    s.account.error = Some(e.to_string());
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_pages_are_opened() {
        for other in ["file:///C:/Windows/System32/calc.exe", "C:/Windows/System32/calc.exe", "//host/share/x.exe", "ms-msdt:/id PCWDiagnostic", "search-ms:query=x", ""] {
            assert!(super::open_url(other).is_err(), "{other}");
        }
    }
}
