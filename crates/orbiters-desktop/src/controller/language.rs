//! The language with the account, on a thread of its own so nothing waits on the network: a pick goes up, a change on the website comes down. Unreachable
//! (or a server without this yet), nothing changes: the last language known stays until the next try.

use super::{Controller, Shared};
use std::time::{Duration, Instant};

/// How often the account is asked: a change on the website shows here within this.
const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How often a pick that did not reach the account tries again.
const SEND_EVERY: Duration = Duration::from_secs(60);

/// What is due now.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Due {
    Nothing,
    /// Signed out: what the account said no longer applies (a pick not sent is the person's, and stays).
    Forget,
    /// A pick goes up.
    Send(Option<String>),
    /// What the account says.
    Ask,
}

/// What the language wants, from what is known. `signed_in`: `None` until the account was read at all: nothing is forgotten on the way up.
pub(super) fn due(signed_in: Option<bool>, language: Option<&str>, unsent: bool, new_account: bool, since_try: Option<Duration>) -> Due {
    match signed_in {
        None => Due::Nothing,
        Some(false) if language.is_some() && !unsent => Due::Forget,
        Some(false) => Due::Nothing,
        Some(true) if unsent => {
            if new_account || since_try.is_none_or(|t| t >= SEND_EVERY) {
                Due::Send(language.map(str::to_string))
            } else {
                Due::Nothing
            }
        }
        Some(true) if new_account || since_try.is_none_or(|t| t >= ASK_EVERY) => Due::Ask,
        Some(true) => Due::Nothing,
    }
}

impl Controller {
    pub(super) fn start_language(self: &Shared) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("language".into()).spawn(move || {
            let mut token: Option<String> = None;
            let mut tried: Option<Instant> = None;
            let mut told = false;
            loop {
                ctl.language_turn(&mut token, &mut tried, &mut told);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    fn language_turn(self: &Shared, token: &mut Option<String>, tried: &mut Option<Instant>, told: &mut bool) {
        // The two fields asked, not a copy of every setting once a second (LIGHT-DESKTOP-15).
        let (language, unsent) = self.with_settings(|s| (s.language.clone(), s.language_unsent));
        let account = self.language_account();
        let signed_in = account.as_ref().map(Option::is_some);
        let now_token = account.as_ref().and_then(|a| a.as_ref().map(|(_, t)| t.clone()));
        let new_account = now_token.is_some() && now_token != *token;
        if account.is_some() {
            *token = now_token;
        }
        let what = due(signed_in, language.as_deref(), unsent, new_account, tried.map(|t| t.elapsed()));
        let Some(Some((api, credential))) = account else {
            if what == Due::Forget {
                self.update_settings(|s| {
                    if !s.language_unsent {
                        s.language = None;
                    }
                });
            }
            return;
        };
        let mut note = |e: anyhow::Error| {
            if !std::mem::replace(told, true) {
                eprintln!("language with the account: {e:#}");
            }
        };
        match what {
            Due::Send(language) => {
                *tried = Some(Instant::now());
                match api.set_language(&credential, language.as_deref()) {
                    // Picked again meanwhile: that one still has to go.
                    Ok(_) => self.update_settings(|s| {
                        if s.language == language {
                            s.language_unsent = false;
                        }
                    }),
                    Err(e) => note(e),
                }
            }
            Due::Ask => {
                *tried = Some(Instant::now());
                match api.language(&credential) {
                    Ok(answer) => {
                        let answer = answer.map(|l| l.tag().to_string());
                        if answer != language {
                            self.update_settings(|s| {
                                if !s.language_unsent {
                                    s.language = answer;
                                }
                            });
                        }
                    }
                    Err(e) => note(e),
                }
            }
            Due::Forget | Due::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATER: Option<Duration> = Some(Duration::from_secs(3600));
    const JUST_NOW: Option<Duration> = Some(Duration::from_secs(1));

    #[test]
    fn nothing_is_forgotten_before_the_account_is_known() {
        assert_eq!(due(None, Some("ja"), false, false, None), Due::Nothing);
    }

    #[test]
    fn signing_out_goes_back_to_the_computer_but_keeps_a_pick_not_sent() {
        assert_eq!(due(Some(false), Some("ja"), false, false, None), Due::Forget);
        assert_eq!(due(Some(false), None, false, false, None), Due::Nothing);
        assert_eq!(due(Some(false), Some("ja"), true, false, None), Due::Nothing);
    }

    #[test]
    fn a_pick_goes_up_and_tries_again_at_its_pace() {
        assert_eq!(due(Some(true), Some("fr"), true, false, None), Due::Send(Some("fr".into())));
        assert_eq!(due(Some(true), None, true, false, None), Due::Send(None), "same as this computer clears the account's");
        assert_eq!(due(Some(true), Some("fr"), true, false, JUST_NOW), Due::Nothing, "the server is down: not every second");
        assert_eq!(due(Some(true), Some("fr"), true, false, LATER), Due::Send(Some("fr".into())));
        assert_eq!(due(Some(true), Some("fr"), true, true, JUST_NOW), Due::Send(Some("fr".into())), "picked before signing in: it goes up when signed in");
    }

    #[test]
    fn the_account_is_asked_on_signing_in_and_now_and_then() {
        assert_eq!(due(Some(true), None, false, true, JUST_NOW), Due::Ask);
        assert_eq!(due(Some(true), Some("ja"), false, false, None), Due::Ask, "at the start");
        assert_eq!(due(Some(true), Some("ja"), false, false, JUST_NOW), Due::Nothing);
        assert_eq!(due(Some(true), Some("ja"), false, false, LATER), Due::Ask);
    }
}
