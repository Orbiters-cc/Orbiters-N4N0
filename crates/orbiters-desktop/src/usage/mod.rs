//! The beta's usage statistics: which controls are pressed and which way people go through the tool, so that the interface can be
//! improved from what people do (Enzo, 2026-11-15). On unless turned off in Settings (`settings.usage.enabled`); a one-time notice says
//! so, and what is never recorded.
//!
//! **What is recorded**, and nothing else: the name of a page or menu entered and of the one it was entered from, the name of a control
//! pressed and the page it was on, when (to the millisecond), and per session the build, the system, the surface (the app's window,
//! the VR overlay, the panels beside VRChat in desktop mode) and the language. Names are the app's own: an enum's name in code
//! ([`stable_name`]: numbers, characters and texts in it are dropped) or a control's English label when it is a text of the catalogs
//! ([`orbiters_i18n::control_name`]). **Never**: anything typed or said, translations, pictures, codes or web addresses, worlds,
//! instances, people, file paths, the VRChat account. An install is a random id ([`spool`]). Signed in to Orbiters, and once the notice
//! saying so was shown (`settings.usage.told_account`), what is counted is linked to the Orbiters account: the batch goes with the
//! account's credential and the server notes whose it is ([`spool::Stamp`]); what was counted before that notice never is.
//!
//! Each process (window, overlay, desktop panels) notes steps here, into a bounded list behind a mutex: no allocation beyond the
//! step itself, nothing on the frame's path but that. A thread of the process sends them to the service every few seconds
//! ([`forward`]); the service adds the session and keeps them on disk, bounded, until Orbiters takes them ([`spool`]).

pub mod perf;
pub mod spool;

use orbiters_core::locks::*;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
#[cfg_attr(test, allow(unused_imports))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// At most this many steps wait in a process for its next send: more is a loop gone wrong, and the oldest go.
const MAX_WAITING: usize = 2_000;
/// How often a process sends what it noted.
const EVERY: Duration = Duration::from_secs(4);
/// The longest name kept: longer is not a name of ours.
pub const MAX_NAME: usize = 96;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A page, menu or sheet was entered (`from`: the one before).
    Enter,
    /// A control was pressed (`screen`: where).
    Press,
}

/// One step, as a process notes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Milliseconds since 1970 (UTC).
    pub at: u64,
    pub kind: Kind,
    pub screen: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
}

struct Noted {
    here: Option<String>,
    steps: Vec<Step>,
}

#[cfg(not(test))]
static ON: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static NOTED: Mutex<Noted> = Mutex::new(Noted { here: None, steps: Vec::new() });

// Tests run side by side, each with panels of its own: each thread notes for itself there.
#[cfg(test)]
thread_local! {
    static ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NOTED: std::cell::RefCell<Noted> = const { std::cell::RefCell::new(Noted { here: None, steps: Vec::new() }) };
}

#[cfg(not(test))]
fn noted<R>(f: impl FnOnce(&mut Noted) -> R) -> R {
    f(&mut NOTED.locked())
}

#[cfg(test)]
fn noted<R>(f: impl FnOnce(&mut Noted) -> R) -> R {
    NOTED.with(|n| f(&mut n.borrow_mut()))
}

#[cfg(not(test))]
fn swap_on(on: bool) -> bool {
    ON.swap(on, Ordering::Relaxed)
}

#[cfg(test)]
fn swap_on(on: bool) -> bool {
    ON.with(|o| o.replace(on))
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Whether steps are noted now (what the settings said last, [`set_enabled`]).
pub fn enabled() -> bool {
    #[cfg(not(test))]
    return ON.load(Ordering::Relaxed);
    #[cfg(test)]
    return ON.with(|o| o.get());
}

/// Follows the setting: turned off, what waits is dropped at once.
pub fn set_enabled(on: bool) {
    if !swap_on(on) || on {
        return;
    }
    noted(|n| n.steps.clear());
}

/// A name as it may leave this computer, or nothing: one line of at most [`MAX_NAME`] bytes, no web address, no long number (an id).
/// Names come from the code and the catalogs alone; this is the last guard.
pub fn clean(name: &str) -> Option<&str> {
    let name = name.trim();
    let mut digits = 0;
    let long_number = name.chars().any(|c| {
        digits = if c.is_ascii_digit() { digits + 1 } else { 0 };
        digits >= 5
    });
    // VRChat's ids (a world, a person, an avatar, a group), a UUID's first part, an e-mail.
    let lower = name.to_ascii_lowercase();
    let vrchat_id = ["wrld_", "usr_", "avtr_", "grp_", "file_", "inst_"].iter().any(|p| lower.contains(p));
    let uuid = lower.as_bytes().windows(13).any(|w| w[8] == b'-' && w[..8].iter().chain(&w[9..]).all(u8::is_ascii_hexdigit));
    let fine = !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.contains("://")
        && !lower.contains("www.")
        && !name.contains('@')
        && !vrchat_id
        && !uuid
        && !long_number
        && !name.chars().any(char::is_control);
    fine.then_some(name)
}

/// The name of an enum value (`{:?}`) without what it holds that is not a name: `Page(Key('a'))` is `Page.Key`, `Capture(3)` is `Capture`,
/// `Set(Language(true))` is `Set.Language.true`. Characters, texts and numbers are dropped: what was typed, which picture, never reach a name.
pub fn stable_name(debug: &str) -> String {
    let mut out = String::new();
    let mut chars = debug.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                // A character or a text: skipped whole, escapes included.
                let mut escaped = false;
                for d in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if d == '\\' {
                        escaped = true;
                    } else if d == c {
                        break;
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut word = String::from(c);
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_alphanumeric() || d == '_' {
                        word.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // A struct's field names (`{ id: 3 }`) say nothing either.
                let field = chars.peek() == Some(&':');
                if !field && !matches!(word.as_str(), "Some" | "None") {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(&word);
                }
            }
            _ => {}
        }
        if out.len() > MAX_NAME {
            out.truncate(MAX_NAME);
            break;
        }
    }
    out
}

fn push(noted: &mut Noted, step: Step) {
    if noted.steps.len() >= MAX_WAITING {
        noted.steps.remove(0);
    }
    noted.steps.push(step);
}

/// `screen` is where the person is now: noted when it is another than before, with the one before.
pub fn enter(screen: &str) {
    if !enabled() {
        return;
    }
    let Some(screen) = clean(screen) else { return };
    noted(|noted| {
        if noted.here.as_deref() == Some(screen) {
            return;
        }
        let from = noted.here.replace(screen.to_string());
        push(noted, Step { at: now_ms(), kind: Kind::Enter, screen: screen.to_string(), from, control: None });
    });
}

/// A control named `control` was pressed where the person is.
pub fn press(control: &str) {
    if !enabled() {
        return;
    }
    let Some(control) = clean(control) else { return };
    noted(|noted| {
        let screen = noted.here.clone().unwrap_or_else(|| "unknown".into());
        push(noted, Step { at: now_ms(), kind: Kind::Press, screen, from: None, control: Some(control.to_string()) });
    });
}

/// A control of the window pressed (`orbiters_ui::widgets::on_press`): its English name.
pub(crate) fn widget_pressed(name: &'static str) {
    press(name);
}

/// What was noted since the last call.
pub fn take() -> Vec<Step> {
    noted(|n| std::mem::take(&mut n.steps))
}

/// Forgets where the person was (a test, or a new session).
#[cfg(test)]
pub fn reset() {
    noted(|n| {
        n.here = None;
        n.steps.clear();
    });
}

/// In a process linked to the service (window, overlay, desktop panels): notes the window's presses, follows the setting and sends what
/// was noted every few seconds, on a thread of its own.
/// Called again (the overlay links anew after SteamVR came back), the new link is the one sent to.
pub fn forward(ctl: crate::backend::Ctl) {
    static LINK: Mutex<Option<crate::backend::Ctl>> = Mutex::new(None);
    static STARTED: std::sync::Once = std::sync::Once::new();
    orbiters_ui::widgets::on_press(widget_pressed);
    set_enabled(ctl.view().settings.usage.enabled);
    *LINK.locked() = Some(ctl);
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("usage".into()).spawn(|| loop {
            std::thread::sleep(EVERY);
            let ctl = LINK.locked().clone();
            if let Some(ctl) = ctl {
                flush(&ctl);
            }
        });
    });
}

/// Sends what waits now (also as a process leaves).
pub fn flush(ctl: &crate::backend::Ctl) {
    set_enabled(ctl.view().settings.usage.enabled);
    let steps = take();
    if enabled() && !steps.is_empty() {
        ctl.usage(steps);
    }
}

/// A screen's name from its parts, `/` between them.
pub fn screen_name<'a>(parts: impl IntoIterator<Item = Cow<'a, str>>) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests;
