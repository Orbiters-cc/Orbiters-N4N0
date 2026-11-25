//! Who has the mouse and the keyboard on a Mac, decided each turn: VRChat, or the Orbiters parts over it.
//!
//! VRChat holds the pointer for as long as its window has the keyboard, its Quick Menu included (the engine's own rule). So the parts are pointed at by taking the keyboard
//! for a moment, with no app coming in front: the engine then lets the pointer go and releases the keys the game held, and takes both back by itself the moment ours is put
//! away. Wanted while Left Option is held with a menu up, and while a page of ours is up.

use super::super::zones::Around;

/// What is so this turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// The game is the app in front (our window having the keyboard does not change that).
    pub front: bool,
    pub open: Option<Around>,
    /// A page of ours is up, and shows (not put aside to reach VRChat's own menus).
    pub sheet: bool,
    /// Left Option is held.
    pub option: bool,
    /// The keyboard we took is no longer ours: the person clicked the game, or another window took it.
    pub lost: bool,
    /// The person gave it back (Esc with a page up).
    pub given_back: bool,
}

#[derive(Debug, Default)]
pub struct Pointing {
    /// The person gave the game its mouse back with the key still held, or the page still up: until the key is pressed again, or a page opens.
    yielded: bool,
    was: Turn,
}

impl Pointing {
    /// Whether the parts have the mouse and the keyboard from now on.
    pub fn turn(&mut self, now: Turn) -> bool {
        let was = std::mem::replace(&mut self.was, now);
        if !now.front || now.open.is_none() {
            self.yielded = false;
            return false;
        }
        if now.lost || now.given_back {
            self.yielded = true;
        }
        // The key pressed, or a page opened: asked anew.
        if (now.option && !was.option) || (now.sheet && !was.sheet) {
            self.yielded = false;
        }
        // A page put away with the key still held: the key still says who has the mouse.
        (now.option || now.sheet) && !self.yielded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHEEL: Turn = Turn { front: true, open: Some(Around::Wheel), sheet: false, option: false, lost: false, given_back: false };

    /// Left Option frees the mouse around both of VRChat's menus, and only while held.
    #[test]
    fn left_option_points_while_held_with_either_menu_up() {
        for open in [Around::Wheel, Around::QuickMenu] {
            let mut p = Pointing::default();
            let turn = Turn { open: Some(open), ..WHEEL };
            assert!(!p.turn(turn), "the game has it");
            assert!(p.turn(Turn { option: true, ..turn }));
            assert!(p.turn(Turn { option: true, ..turn }));
            assert!(!p.turn(turn), "let go: the game's at once");
        }
        let mut p = Pointing::default();
        assert!(!p.turn(Turn { open: None, option: true, ..WHEEL }), "no menu: Option is the game's");
        assert!(!p.turn(Turn { front: false, option: true, ..WHEEL }), "another app in front: Option is that app's");
    }

    /// A page keeps it with no key held, from the press that opened it until it closes.
    #[test]
    fn a_page_keeps_the_mouse_until_it_closes() {
        let mut p = Pointing::default();
        p.turn(WHEEL);
        assert!(p.turn(Turn { option: true, ..WHEEL }));
        assert!(p.turn(Turn { option: true, sheet: true, ..WHEEL }));
        assert!(p.turn(Turn { sheet: true, ..WHEEL }), "the key let go with the page up");
        assert!(!p.turn(WHEEL), "closed");
        // Opened by a number, or by N4N0: no key at all.
        assert!(p.turn(Turn { sheet: true, ..WHEEL }));
    }

    /// A click on the game gives it its mouse back with the page still up or the key still held; the key pressed again, or another page, asks anew.
    #[test]
    fn a_click_on_the_game_gives_it_the_mouse_back() {
        let mut p = Pointing::default();
        let sheet = Turn { sheet: true, ..WHEEL };
        assert!(p.turn(sheet));
        assert!(!p.turn(Turn { lost: true, ..sheet }));
        assert!(!p.turn(sheet), "it stays the game's, the page up");
        assert!(p.turn(Turn { option: true, ..sheet }), "Left Option takes it again");
        assert!(p.turn(sheet));
        assert!(!p.turn(Turn { given_back: true, ..sheet }), "Esc");
        assert!(!p.turn(WHEEL));
        assert!(p.turn(sheet), "a page opened again");
        // The key held through it: one press, one time.
        let mut p = Pointing::default();
        p.turn(WHEEL);
        let held = Turn { option: true, ..WHEEL };
        assert!(p.turn(held));
        assert!(!p.turn(Turn { lost: true, ..held }));
        assert!(!p.turn(held), "still held: it does not fight the person");
        assert!(!p.turn(WHEEL));
        assert!(p.turn(held), "pressed again");
    }

    /// Cmd-Tab away, the menu closed, the game ended: the keyboard is let go at once, and coming back starts clean.
    #[test]
    fn nothing_is_held_once_the_game_is_not_in_front() {
        for gone in [Turn { front: false, option: true, sheet: true, ..WHEEL }, Turn { open: None, option: true, sheet: true, ..WHEEL }] {
            let mut p = Pointing::default();
            p.turn(WHEEL);
            assert!(p.turn(Turn { sheet: true, ..WHEEL }));
            assert!(!p.turn(gone));
            assert!(p.turn(Turn { sheet: true, ..WHEEL }), "back, with the page still up");
        }
        // A click on the game, then away and back: what was given back is forgotten with the app.
        let mut p = Pointing::default();
        p.turn(Turn { sheet: true, ..WHEEL });
        p.turn(Turn { sheet: true, lost: true, ..WHEEL });
        p.turn(Turn { front: false, ..WHEEL });
        assert!(p.turn(Turn { sheet: true, ..WHEEL }));
    }
}
