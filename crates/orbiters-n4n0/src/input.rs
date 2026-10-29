//! Controller input for avatars (contracts `Docs/orb-input.md`): each hand's touchpad and thumbstick, as the SteamVR
//! overlay reads them, sent as N4N0's local parameters to an avatar that lists them. The overlay paces what it hands
//! over ([`Pacer`]); the orb thread sends what changed of the parameters the avatar has ([`InputRuntime`]), the whole
//! state again now and then while something is off its rest, and puts everything back to rest once the overlay is
//! silent.

use orbiters_contracts::orb::input as contract;
use orbiters_orb::text::Message;
use rosc::OscType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A touchpad or a stick as SteamVR read it: where the thumb is (−1 to 1 each way), whether it rests there, presses,
/// and how hard (a touchpad's force, 0 to 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    pub x: f32,
    pub y: f32,
    pub touch: bool,
    pub click: bool,
    #[serde(default)]
    pub force: f32,
}

impl Pad {
    fn at_rest(&self) -> bool {
        *self == Pad::default()
    }

    /// Moved enough since `sent` to go again: a Touch or Click change, or a position or force [`contract::STEP`] away.
    fn moved_from(&self, sent: &Pad) -> bool {
        self.pressed_unlike(sent) || [(self.x, sent.x), (self.y, sent.y), (self.force, sent.force)].iter().any(|(a, b)| (a - b).abs() >= contract::STEP)
    }

    fn pressed_unlike(&self, sent: &Pad) -> bool {
        self.touch != sent.touch || self.click != sent.click
    }
}

/// Both hands' touchpads and sticks, left then right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pads {
    pub touchpads: [Pad; 2],
    pub sticks: [Pad; 2],
}

impl Pads {
    pub fn at_rest(&self) -> bool {
        self.touchpads.iter().chain(&self.sticks).all(Pad::at_rest)
    }

    fn pads(&self) -> impl Iterator<Item = &Pad> {
        self.touchpads.iter().chain(&self.sticks)
    }

    /// Each parameter and its value, as contracts `orb::input::all()` lists them; booleans as 0 or 1.
    fn values(&self) -> impl Iterator<Item = (&'static str, f32)> + '_ {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        let touchpads = (0..2).flat_map(move |h| {
            let p = self.touchpads[h];
            let names = contract::TOUCHPAD[h];
            [(names[contract::X], p.x), (names[contract::Y], p.y), (names[contract::TOUCH], b(p.touch)), (names[contract::CLICK], b(p.click)), (names[contract::FORCE], p.force)]
        });
        let sticks = (0..2).flat_map(move |h| {
            let p = self.sticks[h];
            let names = contract::STICK[h];
            [(names[contract::X], p.x), (names[contract::Y], p.y), (names[contract::TOUCH], b(p.touch)), (names[contract::CLICK], b(p.click))]
        });
        touchpads.chain(sticks)
    }
}

/// The overlay's side: which of the states it reads each turn go to the service. A Touch or Click change goes at once;
/// a move of [`contract::STEP`] or more at most [`contract::RATE_HZ`] times a second; while anything is off its rest,
/// the state again every [`contract::REFRESH_MS`] (the service puts everything back to rest after
/// [`contract::SILENT_MS`] without one).
#[derive(Default)]
pub struct Pacer {
    sent: Option<(Pads, Instant)>,
}

impl Pacer {
    pub fn step(&mut self, now: Instant, pads: Pads) -> Option<Pads> {
        let due = match self.sent {
            None => !pads.at_rest(),
            Some((sent, at)) => {
                let since = now.saturating_duration_since(at);
                let pressed = pads.pads().zip(sent.pads()).any(|(p, s)| p.pressed_unlike(s));
                let moved = pads.pads().zip(sent.pads()).any(|(p, s)| p.moved_from(s));
                pressed
                    || (moved && since >= Duration::from_secs(1) / contract::RATE_HZ)
                    // Back at rest: the last word goes once the rate allows, so the rest is never left out.
                    || (pads.at_rest() && sent != pads && since >= Duration::from_secs(1) / contract::RATE_HZ)
                    || (!pads.at_rest() && since >= Duration::from_millis(contract::REFRESH_MS))
            }
        };
        if due {
            self.sent = Some((pads, now));
            Some(pads)
        } else {
            None
        }
    }

    /// The overlay stops reading (the setting turned off): the state at rest, once, when the last one sent was not.
    pub fn stop(&mut self) -> Option<Pads> {
        match self.sent.take() {
            Some((sent, _)) if !sent.at_rest() => Some(Pads::default()),
            _ => None,
        }
    }
}

/// The orb thread's side.
#[derive(Default)]
pub(crate) struct InputRuntime {
    /// What the overlay last handed over, and when.
    latest: Pads,
    heard: Option<Instant>,
    /// Each parameter's value as last sent to the worn avatar.
    sent: HashMap<&'static str, f32>,
    /// When the whole state last went.
    refreshed: Option<Instant>,
}

impl InputRuntime {
    pub(crate) fn heard(&mut self, pads: Pads, now: Instant) {
        self.latest = pads;
        self.heard = Some(now);
    }

    /// Another avatar (or none): it gets the state afresh.
    pub(crate) fn forget_avatar(&mut self) {
        self.sent.clear();
        self.refreshed = None;
    }

    /// How many of the input parameters `present` (the avatar's) lists.
    pub(crate) fn used(present: &HashSet<String>) -> usize {
        contract::all().filter(|n| present.contains(*n)).count()
    }

    /// The messages to send now to an avatar with the parameters `present` (none while they are not known): what changed,
    /// or every value off its rest again when a refresh is due. `enabled` off, or the overlay silent, puts every value back
    /// to rest.
    pub(crate) fn tick(&mut self, now: Instant, enabled: bool, present: &HashSet<String>) -> Vec<Message> {
        if !enabled || self.heard.is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(contract::SILENT_MS)) {
            self.latest = Pads::default();
            self.heard = None;
        }
        let refresh = !self.latest.at_rest() && self.refreshed.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(contract::REFRESH_MS));
        if refresh {
            self.refreshed = Some(now);
        }
        let mut out = Vec::new();
        for (name, value) in self.latest.values() {
            if !present.contains(name) {
                continue;
            }
            // Never sent counts as at rest: the avatar's default.
            let sent = self.sent.get(name).copied().unwrap_or(0.0);
            if sent != value || (refresh && value != 0.0) {
                self.sent.insert(name, value);
                let arg = if contract::is_bool(name) { OscType::Bool(value != 0.0) } else { OscType::Float(value) };
                out.push((crate::runtime::parameter(name), arg));
            }
        }
        out
    }

    /// When the next tick has something to do without news from the overlay: the refresh, or the rest after silence.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        if self.latest.at_rest() {
            return None;
        }
        let refresh = self.refreshed.map(|t| t + Duration::from_millis(contract::REFRESH_MS));
        let silent = self.heard.map(|t| t + Duration::from_millis(contract::SILENT_MS));
        refresh.into_iter().chain(silent).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> HashSet<String> {
        contract::all().map(String::from).collect()
    }

    fn thumb(x: f32, y: f32) -> Pads {
        let mut p = Pads::default();
        p.touchpads[1] = Pad { x, y, touch: true, click: false, force: 0.0 };
        p
    }

    #[test]
    fn the_pacer_sends_presses_at_once_moves_at_its_rate_and_the_rest_once() {
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        assert_eq!(pacer.step(t0, Pads::default()), None, "nothing to say at rest");
        assert!(pacer.step(t0, thumb(0.5, 0.0)).is_some(), "a touch goes at once");
        assert_eq!(pacer.step(t0 + Duration::from_millis(5), thumb(0.6, 0.0)), None, "a move waits for the rate");
        assert!(pacer.step(t0 + Duration::from_millis(40), thumb(0.6, 0.0)).is_some());
        assert_eq!(pacer.step(t0 + Duration::from_millis(80), thumb(0.605, 0.0)), None, "less than a step");
        assert!(pacer.step(t0 + Duration::from_millis(1041), thumb(0.605, 0.0)).is_some(), "again after a second off rest");
        let mut clicked = thumb(0.605, 0.0);
        clicked.touchpads[1].click = true;
        assert!(pacer.step(t0 + Duration::from_millis(1042), clicked).is_some(), "a click goes at once");
        assert!(pacer.step(t0 + Duration::from_millis(1043), Pads::default()).is_some(), "letting go is a press change");
        assert_eq!(pacer.step(t0 + Duration::from_millis(3000), Pads::default()), None, "at rest stays quiet");
        // A stick let go slowly ends at rest even when its last move was under a step.
        let mut stick = Pads::default();
        stick.sticks[0] = Pad { x: 0.005, y: 0.0, touch: true, click: false, force: 0.0 };
        assert!(pacer.step(t0 + Duration::from_millis(4000), stick).is_some());
        stick.sticks[0].touch = false;
        stick.sticks[0].x = 0.0;
        assert!(pacer.step(t0 + Duration::from_millis(4100), stick).is_some());
        assert_eq!(pacer.stop(), None, "the rest was the last word");
        assert!(pacer.step(t0 + Duration::from_millis(5000), thumb(0.1, 0.1)).is_some());
        assert_eq!(pacer.stop(), Some(Pads::default()));
    }

    #[test]
    fn only_the_avatars_parameters_go_and_only_what_changed() {
        let t0 = Instant::now();
        let mut rt = InputRuntime::default();
        let some: HashSet<String> = [contract::TOUCHPAD[1][contract::X], contract::TOUCHPAD[1][contract::TOUCH]].iter().map(|s| s.to_string()).collect();
        rt.heard(thumb(0.5, 0.25), t0);
        let out = rt.tick(t0, true, &some);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().any(|(n, v)| n == "/avatar/parameters/N4N0/Touchpad/Right/X" && *v == OscType::Float(0.5)));
        assert!(out.iter().any(|(n, v)| n == "/avatar/parameters/N4N0/Touchpad/Right/Touch" && *v == OscType::Bool(true)));
        assert!(rt.tick(t0 + Duration::from_millis(10), true, &some).is_empty(), "nothing changed");
        rt.heard(thumb(0.7, 0.9), t0 + Duration::from_millis(40));
        let out = rt.tick(t0 + Duration::from_millis(40), true, &some);
        assert_eq!(out, vec![("/avatar/parameters/N4N0/Touchpad/Right/X".to_string(), OscType::Float(0.7))], "Y is not the avatar's");
        assert!(rt.tick(t0, true, &HashSet::new()).is_empty(), "parameters not known: nothing");
        assert_eq!(InputRuntime::used(&some), 2);
        assert_eq!(InputRuntime::used(&all()), 18);
    }

    #[test]
    fn a_refresh_repeats_what_is_off_rest_and_silence_puts_everything_back() {
        let t0 = Instant::now();
        let mut rt = InputRuntime::default();
        rt.heard(thumb(0.5, 0.0), t0);
        let first = rt.tick(t0, true, &all());
        assert_eq!(first.len(), 2, "X and Touch off their rest: {first:?}");
        assert_eq!(rt.next_due(), Some(t0 + Duration::from_millis(contract::REFRESH_MS)));
        let again = rt.tick(t0 + Duration::from_millis(contract::REFRESH_MS), true, &all());
        assert_eq!(again, first, "the refresh");
        let rest = rt.tick(t0 + Duration::from_millis(contract::SILENT_MS), true, &all());
        assert_eq!(rest.len(), 2);
        assert!(rest.contains(&("/avatar/parameters/N4N0/Touchpad/Right/X".to_string(), OscType::Float(0.0))));
        assert!(rest.contains(&("/avatar/parameters/N4N0/Touchpad/Right/Touch".to_string(), OscType::Bool(false))));
        assert_eq!(rt.next_due(), None);
        // A new avatar gets the state afresh; turned off, it goes back to rest.
        rt.heard(thumb(0.2, 0.0), t0 + Duration::from_secs(5));
        assert_eq!(rt.tick(t0 + Duration::from_secs(5), true, &all()).len(), 2);
        rt.forget_avatar();
        assert_eq!(rt.tick(t0 + Duration::from_secs(5), true, &all()).len(), 2);
        assert_eq!(rt.tick(t0 + Duration::from_secs(5), false, &all()).len(), 2);
        assert!(rt.tick(t0 + Duration::from_secs(6), false, &all()).is_empty());
    }

    #[test]
    fn values_follow_the_contracts_order() {
        let names: Vec<&str> = Pads::default().values().map(|(n, _)| n).collect();
        assert_eq!(names, contract::all().collect::<Vec<_>>());
    }
}
