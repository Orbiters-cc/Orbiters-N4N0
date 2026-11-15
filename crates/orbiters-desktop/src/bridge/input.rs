//! Input as the bridge holds it: what it may press, what is down right now, and how it comes up again. Nothing is ever
//! left down by design: a press lives inside one tool call, for a bounded time, and comes up when the call ends, when its
//! client goes away, when the person takes over, and (the sweep) when anything else went wrong. A process that dies with
//! something down has the guard (`guard.rs`).

use super::game::{Failure, OscArg};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Keyboard and mouse input for the game's window. Mouse positions are the window's own pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    Key {
        scan: u16,
        extended: bool,
        down: bool,
    },
    /// Moves the mouse by so many counts: how the game looks around when no menu is open.
    MoveBy {
        dx: i32,
        dy: i32,
    },
    /// Moves the pointer over a point of the window: for menus.
    MoveTo {
        x: u32,
        y: u32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Something that is down until it is released.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hold {
    /// An OSC button: `/input/…`, 1 while down.
    OscButton(&'static str),
    /// An OSC axis: `/input/…`, 0 at rest.
    OscAxis(&'static str),
    Key {
        scan: u16,
        extended: bool,
    },
    Button(MouseButton),
}

impl Hold {
    /// What brings it up, when it is keyboard or mouse.
    pub fn up(&self) -> Option<Input> {
        match *self {
            Hold::Key { scan, extended } => Some(Input::Key { scan, extended, down: false }),
            Hold::Button(button) => Some(Input::Button { button, down: false }),
            Hold::OscButton(_) | Hold::OscAxis(_) => None,
        }
    }

    /// What brings it up, when it is OSC.
    pub fn osc_rest(&self) -> Option<(&'static str, OscArg)> {
        match *self {
            Hold::OscButton(address) => Some((address, OscArg::Int(0))),
            Hold::OscAxis(address) => Some((address, OscArg::Float(0.0))),
            _ => None,
        }
    }

    /// The line the guard reads for it (`guard.rs`).
    pub fn guard_line(&self) -> String {
        match self {
            Hold::OscButton(a) => format!("osc-button {a}"), // i18n: not shown (the bridge's log)
            Hold::OscAxis(a) => format!("osc-axis {a}"), // i18n: not shown (the bridge's log)
            Hold::Key { scan, extended } => format!("key {scan} {extended}"), // i18n: not shown (the bridge's log)
            Hold::Button(MouseButton::Left) => "button left".into(), // i18n: not shown
            Hold::Button(MouseButton::Right) => "button right".into(), // i18n: not shown
            Hold::Button(MouseButton::Middle) => "button middle".into(), // i18n: not shown
        }
    }
}

/// An OSC input an agent may send, by the name it knows it. VRChat's `/input/*` list (docs.vrchat.com/docs/osc-as-input-controller)
/// without `/input/Voice` (the microphone), the hands' VR-only buttons and `/chatbox/*`.
pub const OSC_INPUTS: [(&str, &str, bool); 5] = [
    ("jump", "/input/Jump", false),
    ("quick_menu", "/input/QuickMenuToggleLeft", false),
    ("look_horizontal", "/input/LookHorizontal", true),
    ("move_vertical", "/input/Vertical", true),
    ("move_horizontal", "/input/Horizontal", true),
];

/// The keys an agent may press: movement, the Esc menu, the Action Menu, and what worlds commonly ask for on their own
/// screens and in their games (Tab on SlashCo VR's language page, met on the first world tour on 2026-11-15; E, F and Q to
/// use and pick up; digits for a choice). None of them is bound by VRChat's desktop mode to anything outside the world. The
/// chatbox (`Y`), push-to-talk (`V`), Enter, Backspace, the F keys (VRChat's hand gestures, Steam's screenshot), Ctrl and Alt
/// (VRChat's shortcuts, Alt+F4) and anything that combines stay out: only one key at a time, by these names. A letter or a
/// digit types only into a text field that is open, and nothing here opens one (VRChat's keyboard needs a click on a field).
pub const KEYS: [(&str, u16, bool); 28] = [
    ("esc", 0x01, false),
    ("r", 0x13, false),
    ("w", 0x11, false),
    ("a", 0x1E, false),
    ("s", 0x1F, false),
    ("d", 0x20, false),
    ("space", 0x39, false),
    ("shift", 0x2A, false),
    ("c", 0x2E, false),
    ("z", 0x2C, false),
    ("up", 0x48, true),
    ("down", 0x50, true),
    ("left", 0x4B, true),
    ("right", 0x4D, true),
    ("tab", 0x0F, false),
    ("e", 0x12, false),
    ("f", 0x21, false),
    ("q", 0x10, false),
    ("1", 0x02, false),
    ("2", 0x03, false),
    ("3", 0x04, false),
    ("4", 0x05, false),
    ("5", 0x06, false),
    ("6", 0x07, false),
    ("7", 0x08, false),
    ("8", 0x09, false),
    ("9", 0x0A, false),
    ("0", 0x0B, false),
];

/// The names [`press`](super::tools) takes, for its description and its errors.
pub fn input_names() -> Vec<&'static str> {
    OSC_INPUTS.iter().map(|i| i.0).chain(KEYS.iter().map(|k| k.0)).collect()
}

/// What is down now, and until when at the latest.
#[derive(Default)]
pub struct Held {
    inner: Mutex<HeldInner>,
}

#[derive(Default)]
struct HeldInner {
    down: HashSet<Hold>,
    until: Option<Instant>,
}

impl Held {
    /// `hold` went down, to be up by `until` at the latest. Whether it was up before.
    pub fn press(&self, hold: Hold, until: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.until = Some(inner.until.map_or(until, |t| t.max(until)));
        inner.down.insert(hold)
    }

    /// `hold` came up. Whether it was down.
    pub fn lift(&self, hold: &Hold) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let was = inner.down.remove(hold);
        if inner.down.is_empty() {
            inner.until = None;
        }
        was
    }

    /// Everything down, emptied: the caller brings them up.
    pub fn take_all(&self) -> Vec<Hold> {
        let mut inner = self.inner.lock().unwrap();
        inner.until = None;
        inner.down.drain().collect()
    }

    /// What should be up by now and is not.
    pub fn overdue(&self, now: Instant) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.down.is_empty() && inner.until.is_none_or(|t| now > t + OVERDUE_MARGIN)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().down.is_empty()
    }
}

/// How late a press may run before the sweep brings it up: its own call normally has by then.
const OVERDUE_MARGIN: Duration = Duration::from_millis(500);

/// The longest anything stays down in one call.
pub const LONGEST_HOLD: Duration = Duration::from_secs(10);

/// The press a tool asks for, checked: how long, bounded, or a failure saying the limit.
pub fn bounded_seconds(seconds: f64) -> Result<Duration, Failure> {
    if !(seconds.is_finite() && seconds > 0.0) {
        return Err(Failure::new("bad_duration", "seconds must be a positive number.", "Pass seconds between 0.05 and 10.")); // i18n: not shown (an AI client reads it in the tool's answer)
    }
    if seconds > LONGEST_HOLD.as_secs_f64() {
        return Err(Failure::new(
            "bad_duration",
            format!("{seconds} s is longer than the {} s a press may last: a lost connection must not leave an avatar walking.", LONGEST_HOLD.as_secs()), // i18n: not shown (an AI client reads it in the tool's answer)
            "Split it into several calls of 10 s or less.", // i18n: not shown (an AI client reads it in the tool's answer)
        ));
    }
    Ok(Duration::from_secs_f64(seconds.max(0.05)))
}

/// Turning: how long `/input/LookLeft` or `Right` stays down for about `degrees`, at about `rate` degrees a second.
pub fn turn_seconds(degrees: f64, rate: f64) -> f64 {
    degrees.abs() / rate.max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_that_can_type_or_speak_is_on_the_lists() {
        let names = input_names();
        for name in ["y", "enter", "t", "v", "/chatbox/input", "voice", "panic", "backspace", "ctrl", "alt", "f12", "f1", "m"] {
            assert!(!names.contains(&name), "{name}");
        }
        assert!(OSC_INPUTS.iter().all(|i| i.1.starts_with("/input/") && !i.1.contains("Voice")));
        // Each name once, each scan code once.
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        let mut scans: Vec<u16> = KEYS.iter().map(|k| k.1).collect();
        scans.sort_unstable();
        scans.dedup();
        assert_eq!(scans.len(), KEYS.len(), "each scan code once");
        // What worlds ask for on their own entry screens (SlashCo VR: "[PC: TAB]").
        for name in ["tab", "e", "f", "q", "1", "0"] {
            assert!(names.contains(&name), "{name}");
        }
        assert_eq!(KEYS.iter().find(|k| k.0 == "tab").map(|k| (k.1, k.2)), Some((0x0F, false)), "Tab's scan code");
    }

    #[test]
    fn what_is_held_is_known_until_it_comes_up() {
        let held = Held::default();
        let now = Instant::now();
        let w = Hold::Key { scan: 0x11, extended: false };
        assert!(held.press(w.clone(), now + Duration::from_secs(1)));
        assert!(!held.press(w.clone(), now + Duration::from_secs(1)), "already down");
        assert!(held.press(Hold::OscButton("/input/MoveForward"), now + Duration::from_secs(2)));
        assert!(!held.overdue(now));
        assert!(!held.overdue(now + Duration::from_secs(2)), "within the margin");
        assert!(held.overdue(now + Duration::from_secs(3)), "the sweep's turn");
        assert!(held.lift(&w));
        assert!(!held.lift(&w));
        assert_eq!(held.take_all(), vec![Hold::OscButton("/input/MoveForward")]);
        assert!(held.is_empty());
        assert!(!held.overdue(now + Duration::from_secs(9)), "nothing to do");
    }

    #[test]
    fn a_press_is_bounded_and_says_so() {
        assert_eq!(bounded_seconds(2.0).unwrap(), Duration::from_secs(2));
        assert_eq!(bounded_seconds(0.001).unwrap(), Duration::from_millis(50), "long enough for a game to see it");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 11.0] {
            assert_eq!(bounded_seconds(bad).unwrap_err().code, "bad_duration", "{bad}");
        }
    }

    #[test]
    fn every_hold_has_one_way_up() {
        for hold in [Hold::OscButton("/input/Jump"), Hold::OscAxis("/input/Vertical"), Hold::Key { scan: 1, extended: false }, Hold::Button(MouseButton::Left)]
        {
            assert!(hold.up().is_some() != hold.osc_rest().is_some(), "{hold:?}");
        }
        assert_eq!(Hold::OscAxis("/input/Vertical").osc_rest(), Some(("/input/Vertical", OscArg::Float(0.0))));
        assert_eq!(Hold::OscButton("/input/Jump").osc_rest(), Some(("/input/Jump", OscArg::Int(0))));
    }
}
