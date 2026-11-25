//! The keys of a Mac, read and never taken: VRChat's own keys (Esc, R), the numbers and Left Option, by asking macOS for their state each turn
//! (`CGEventSourceKeyState`), only while the game is in front. No event tap, no hot key (which would keep the key from the game), no Accessibility. And what a key types
//! in a page of ours, in the person's layout, for the keys macOS gives our own window while it holds the keyboard.

use super::super::detect::MenuKey;
use std::ffi::c_void;
use std::time::{Duration, Instant};

type CFTypeRef = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGEventSourceCounterForEventType(state: i32, kind: u32) -> u32;
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(layout: *const c_void, key: u16, action: u16, modifiers: u32, keyboard: u32, options: u32, dead: *mut u32, most: usize, length: *mut usize, out: *mut u16) -> i32;
    static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

/// `kCGEventSourceStateHIDSystemState`: what the keyboard itself says, whatever any program posts.
const HID: i32 = 1;
/// `kCGEventKeyDown`.
const KEY_DOWN: u32 = 10;
/// Virtual key codes (`HIToolbox/Events.h`), the same places on every keyboard.
pub const ESC: u16 = 0x35;
pub const LEFT_OPTION: u16 = 0x3a;
/// The top row's 1 to 9, as they lie on an ANSI, ISO or JIS keyboard, whatever the layout writes there (an AZERTY's digits need Shift, as Windows' own keys do).
const DIGITS: [u16; 9] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19];
/// Keys that are modifiers: macOS says their state whatever it hides of the others.
const MODIFIERS: std::ops::RangeInclusive<u16> = 0x36..=0x3f;
/// R where no layout can be read: its place on QWERTY, QWERTZ and AZERTY.
const R: u16 = 0x0f;

/// The keys as they are now.
pub trait Source {
    fn down(&self, key: u16) -> bool;
    /// How many times any key went down since the Mac started (it counts whatever may be seen of which).
    fn downs(&self) -> u32;
    /// The left mouse button.
    fn button(&self) -> bool;
    /// Where the key that types "r" is.
    fn r(&self) -> u16;
    /// Whether macOS said this app may hear the keyboard (Input Monitoring).
    fn allowed(&self) -> bool;
}

/// This Mac's keyboard.
pub struct Native {
    r: std::cell::Cell<(u16, Option<Instant>)>,
}

impl Default for Native {
    fn default() -> Self {
        Self { r: std::cell::Cell::new((R, None)) }
    }
}

impl Source for Native {
    fn down(&self, key: u16) -> bool {
        // SAFETY: a plain query.
        unsafe { CGEventSourceKeyState(HID, key) }
    }

    fn downs(&self) -> u32 {
        // SAFETY: a plain query.
        unsafe { CGEventSourceCounterForEventType(HID, KEY_DOWN) }
    }

    fn button(&self) -> bool {
        // SAFETY: a plain query.
        unsafe { CGEventSourceButtonState(HID, 0) }
    }

    fn r(&self) -> u16 {
        let (key, asked) = self.r.get();
        if asked.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return key;
        }
        let key = Layout::now().and_then(|l| l.key_of('r')).unwrap_or(R);
        self.r.set((key, Some(Instant::now())));
        key
    }

    fn allowed(&self) -> bool {
        listening_allowed()
    }
}

/// Whether macOS lets this app hear the keyboard (Input Monitoring): no question is shown.
pub fn listening_allowed() -> bool {
    // SAFETY: a plain query: `kIOHIDRequestTypeListenEvent` (1), `kIOHIDAccessTypeGranted` (0).
    unsafe { IOHIDCheckAccess(1) == 0 }
}

/// Asks macOS for Input Monitoring for this app: its own question the first time, else its entry in System Settings, opened for the person to switch on.
pub fn ask_listening() {
    // SAFETY: a plain call; macOS shows its own question at most once.
    let allowed = unsafe { IOHIDRequestAccess(1) };
    if !allowed {
        let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent").spawn();
    }
}

/// What the keys said this turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Said {
    /// Esc or R went down (Esc before R when both).
    pub menu: Option<MenuKey>,
    /// A key went down and up again between two turns, or one macOS does not show: which it was is not known.
    pub unknown: bool,
    pub option: bool,
}

/// From the keys' states, turn by turn, to what was pressed.
#[derive(Debug, Default)]
pub struct Keys {
    was: [bool; 2],
    digits: [bool; 9],
    downs: Option<u32>,
    /// Keys going down that could not be seen, in a row, and when the last one was.
    unseen: u32,
    unseen_at: Option<Instant>,
    /// A key that is no modifier was seen down once: macOS shows them.
    seen: bool,
}

/// Presses in a row that could not be seen before macOS is taken to hide the keys.
const HIDDEN_AFTER: u32 = 8;
/// With keys hidden, only a press this long after the one before starts a look: walking (a key held repeats, and steers) must not keep one running.
const APART: Duration = Duration::from_millis(500);

impl Keys {
    /// A turn with the game in front.
    pub fn turn(&mut self, source: &dyn Source, now: Instant) -> Said {
        let mut menu = None;
        for (was, (code, key)) in self.was.iter_mut().zip([(ESC, MenuKey::Esc), (source.r(), MenuKey::R)]) {
            let down = source.down(code);
            if down && !*was {
                menu = menu.or(Some(key));
            }
            *was = down;
        }
        let downs = source.downs();
        let went = downs.wrapping_sub(self.downs.replace(downs).unwrap_or(downs));
        let mut unknown = false;
        if went > 0 {
            if (0..128).filter(|k| !MODIFIERS.contains(k)).any(|k| source.down(k)) {
                self.seen = true;
                self.unseen = 0;
            } else {
                self.unseen = self.unseen.saturating_add(went.min(HIDDEN_AFTER));
                unknown = self.unseen_at.is_none_or(|at| now.duration_since(at) >= APART) || (self.seen && menu.is_none());
                self.unseen_at = Some(now);
            }
        }
        Said { menu, unknown, option: source.down(LEFT_OPTION) }
    }

    /// Another program in front: what is pressed there is not for the game, and a key still down when it comes back counts for nothing.
    pub fn away(&mut self, source: &dyn Source) {
        self.was = [source.down(ESC), source.down(source.r())];
        self.downs = Some(source.downs());
        self.digits = [true; 9];
    }

    /// Whether macOS hides which keys are pressed: keys went down with the game in front and none of them was ever seen, and it has not said this app may hear them.
    pub fn hidden(&self, source: &dyn Source) -> bool {
        !self.seen && self.unseen >= HIDDEN_AFTER && !source.allowed()
    }

    /// A number from 1 to 9 just pressed, as an index, when `live`. A key still down from before it was live counts for nothing.
    pub fn digit(&mut self, source: &dyn Source, live: bool) -> Option<usize> {
        if !live {
            self.digits = [true; 9];
            return None;
        }
        let mut pressed = None;
        for (i, was) in self.digits.iter_mut().enumerate() {
            let down = source.down(DIGITS[i]);
            if down && !*was {
                pressed = pressed.or(Some(i));
            }
            *was = down;
        }
        pressed
    }
}

/// The person's keyboard layout now (main thread: macOS's input sources are the main thread's).
pub struct Layout {
    source: CFTypeRef,
    data: *const c_void,
}

impl Layout {
    pub fn now() -> Option<Self> {
        objc2::MainThreadMarker::new()?;
        // SAFETY: plain queries on the main thread; the source is ours (copied) and released on drop, and its data lives as long.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            let data: *const c_void = if data.is_null() { std::ptr::null() } else { CFDataGetBytePtr(data).cast() };
            if data.is_null() {
                CFRelease(source);
                return None;
            }
            Some(Self { source, data })
        }
    }

    /// What key `key` writes with `modifiers` (Carbon's, shifted: 2 Shift, 4 Caps Lock, 8 Option), a dead key's accent kept in `dead` for the next.
    pub fn writes(&self, key: u16, modifiers: u32, dead: &mut u32) -> String {
        let mut out = [0u16; 8];
        let mut length = 0usize;
        // SAFETY: the layout's data lives with its source; the buffers are ours, their sizes passed. `kUCKeyActionDown` is 0.
        let status = unsafe { UCKeyTranslate(self.data, key, 0, modifiers, u32::from(LMGetKbdType()), 0, dead, out.len(), &mut length, out.as_mut_ptr()) };
        if status != 0 {
            return String::new();
        }
        String::from_utf16_lossy(&out[..length.min(out.len())])
    }

    /// The key that types `letter` with nothing held.
    pub fn key_of(&self, letter: char) -> Option<u16> {
        (0..51u16).find(|key| self.writes(*key, 0, &mut 0).chars().eq([letter]))
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        // SAFETY: the source was copied in `now` and is released once.
        unsafe { CFRelease(self.source) };
    }
}

/// What a key pressed in a page of ours does to its text field, from its code, the modifiers held and what the layout writes (`None`: not one for the page).
pub fn typed(key: u16, command: bool, control: bool, writes: impl FnOnce() -> String) -> Option<orbiters_n4n0_core::menu::KeyInput> {
    use orbiters_n4n0_core::menu::KeyInput;
    if command {
        // Cmd+V; every other shortcut is macOS's, or nothing.
        return (writes() == "v").then_some(KeyInput::Paste);
    }
    if control {
        return None;
    }
    match key {
        0x33 | 0x75 => return Some(KeyInput::Backspace),
        0x24 | 0x4c => return Some(KeyInput::Enter),
        ESC => return Some(KeyInput::Escape),
        // Tab, the arrows and the function keys write control characters or Apple's own private ones.
        0x30 | 0x7b..=0x7e => return None,
        _ => {}
    }
    let text: String = writes().chars().filter(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(c)).collect();
    (!text.is_empty()).then_some(KeyInput::Text(text))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A keyboard a test presses: `hides` as macOS does without Input Monitoring.
    #[derive(Default)]
    pub struct Scripted {
        pub held: RefCell<Vec<u16>>,
        pub downs: Cell<u32>,
        pub hides: Cell<bool>,
        pub allowed: Cell<bool>,
        pub button: Cell<bool>,
    }

    impl Scripted {
        pub fn press(&self, key: u16) {
            self.held.borrow_mut().push(key);
            self.downs.set(self.downs.get() + 1);
        }

        pub fn let_go(&self, key: u16) {
            self.held.borrow_mut().retain(|k| *k != key);
        }
    }

    impl Source for Scripted {
        fn down(&self, key: u16) -> bool {
            self.held.borrow().contains(&key) && (!self.hides.get() || MODIFIERS.contains(&key))
        }
        fn downs(&self) -> u32 {
            self.downs.get()
        }
        fn button(&self) -> bool {
            self.button.get()
        }
        fn r(&self) -> u16 {
            R
        }
        fn allowed(&self) -> bool {
            self.allowed.get()
        }
    }

    #[test]
    fn a_key_counts_once_as_it_goes_down() {
        let (source, mut keys, t) = (Scripted::default(), Keys::default(), Instant::now());
        assert_eq!(keys.turn(&source, t), Said::default());
        source.press(ESC);
        assert_eq!(keys.turn(&source, t).menu, Some(MenuKey::Esc));
        assert_eq!(keys.turn(&source, t).menu, None, "held");
        source.let_go(ESC);
        assert_eq!(keys.turn(&source, t).menu, None);
        source.press(R);
        source.press(LEFT_OPTION);
        let said = keys.turn(&source, t);
        assert_eq!((said.menu, said.option), (Some(MenuKey::R), true));
        assert!(!said.unknown, "what was pressed is seen");
    }

    /// Typed in another program, it is not VRChat's; nor is a key still held as the game comes back in front.
    #[test]
    fn a_key_pressed_elsewhere_is_not_the_game_s() {
        let (source, mut keys, t) = (Scripted::default(), Keys::default(), Instant::now());
        keys.turn(&source, t);
        source.press(R);
        source.press(0x12);
        keys.away(&source);
        assert_eq!(keys.turn(&source, t), Said::default());
        assert_eq!(keys.digit(&source, true), None, "a digit held from before");
        source.let_go(0x12);
        assert_eq!(keys.digit(&source, true), None);
        source.press(0x12);
        assert_eq!(keys.digit(&source, true), Some(0));
        assert_eq!(keys.digit(&source, true), None, "once");
        assert_eq!(keys.digit(&source, false), None);
    }

    /// A press shorter than a turn: the count moved and nothing is down, which starts a look all the same.
    #[test]
    fn a_press_between_two_turns_is_not_lost() {
        let (source, mut keys, t) = (Scripted::default(), Keys::default(), Instant::now());
        keys.turn(&source, t);
        source.press(0x00);
        keys.turn(&source, t);
        source.let_go(0x00);
        source.downs.set(source.downs.get() + 1);
        let said = keys.turn(&source, t + Duration::from_millis(16));
        assert!(said.unknown && said.menu.is_none());
        assert!(!keys.hidden(&source), "keys are shown here: one was seen");
    }

    /// macOS hiding the keys: said once enough presses went by unseen, only while it has not said it allows them, and a press apart from the others still starts a look;
    /// keys that follow one another (walking) do not. Allowed and seen, it is over.
    #[test]
    fn keys_macos_hides_are_noticed_and_isolated_presses_still_start_a_look() {
        let (source, mut keys, mut t) = (Scripted::default(), Keys::default(), Instant::now());
        source.hides.set(true);
        keys.turn(&source, t);
        let mut looks = 0;
        for _ in 0..HIDDEN_AFTER {
            t += Duration::from_millis(700);
            source.press(0x00);
            looks += u32::from(keys.turn(&source, t).unknown);
            source.let_go(0x00);
        }
        assert_eq!(looks, HIDDEN_AFTER, "each stands apart");
        assert!(keys.hidden(&source));
        let mut walking = 0;
        for _ in 0..40 {
            t += Duration::from_millis(33);
            source.press(0x0d);
            walking += u32::from(keys.turn(&source, t).unknown);
        }
        assert!(walking <= 1, "a key held repeats: {walking} looks");
        source.allowed.set(true);
        assert!(!keys.hidden(&source), "allowed: nothing to ask for");
        source.allowed.set(false);
        source.hides.set(false);
        source.press(0x02);
        keys.turn(&source, t);
        assert!(!keys.hidden(&source), "seen once: shown");
        // Left Option is a modifier, shown whatever is hidden.
        source.hides.set(true);
        source.press(LEFT_OPTION);
        assert!(keys.turn(&source, t).option);
    }

    #[test]
    fn what_a_key_types_in_a_page() {
        use orbiters_n4n0_core::menu::KeyInput;
        let says = |s: &str| {
            let s = s.to_string();
            move || s
        };
        assert_eq!(typed(0x00, false, false, says("q")), Some(KeyInput::Text("q".into())));
        assert_eq!(typed(0x21, false, false, says("ê")), Some(KeyInput::Text("ê".into())));
        assert_eq!(typed(0x21, false, false, says("")), None, "a dead key alone writes nothing yet");
        assert_eq!(typed(0x33, false, false, says("\u{8}")), Some(KeyInput::Backspace));
        assert_eq!(typed(0x24, false, false, says("\r")), Some(KeyInput::Enter));
        assert_eq!(typed(ESC, false, false, says("\u{1b}")), Some(KeyInput::Escape));
        assert_eq!(typed(0x09, true, false, says("v")), Some(KeyInput::Paste));
        assert_eq!(typed(0x08, true, false, says("c")), None);
        assert_eq!(typed(0x00, false, true, says("\u{1}")), None);
        assert_eq!(typed(0x7b, false, false, says("\u{1c}")), None, "an arrow");
        assert_eq!(typed(0x7a, false, false, says("\u{f704}")), None, "F1");
        assert_eq!(typed(0x31, false, false, says(" ")), Some(KeyInput::Text(" ".into())));
    }

    /// Nothing here may take a key from the game, or need Accessibility: no event tap, no hot key. (What is checked is the source of the whole Mac module.)
    #[test]
    fn no_key_is_ever_taken() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/desktop_mac");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            // (Put together, so that this very test does not name what it forbids.)
            for forbidden in [["CGEvent", "TapCreate"].concat(), ["RegisterEvent", "HotKey"].concat(), ["CGEvent", "Post"].concat(), ["AXIs", "ProcessTrusted"].concat(), ["addGlobalMonitor", "ForEvents"].concat()] {
                assert!(!text.contains(&forbidden), "{}: {forbidden}", path.display());
            }
        }
    }
}
