//! Who has the mouse and the keyboard on a Mac, decided each turn: VRChat, or the Orbiters parts over it.
//!
//! **The engine lets the mouse go where VRChat does** (obelisk-cli 0.1.15) and says so ([`super::channel`]): with the Quick Menu open the mouse is the person's, and a part of
//! ours under it takes the click as any window does. No key, and the game keeps its keyboard: whoever walks, walks on. The keyboard is ours only while something of ours is
//! typed in, and for a page opened from VRChat's wheel, which holds the mouse as long as it is up: the engine lets it go to whoever has the keyboard.
//!
//! **An engine from before holds the mouse** for as long as its game has the keyboard, the Quick Menu included, and so does one whose word of it is not there. The parts are
//! then pointed at by taking the keyboard for a moment, with no app coming in front: wanted while Left Option is held with a menu up, and while a page of ours is up.

use super::super::zones::Around;
use super::channel::Pointer;

/// What is so this turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// The game is the app in front (our window having the keyboard does not change that).
    pub front: bool,
    pub open: Option<Around>,
    /// A page of ours is up, and shows (not put aside to reach VRChat's own menus).
    pub sheet: bool,
    /// Something of ours is typed in.
    pub typing: bool,
    /// Left Option is held.
    pub option: bool,
    /// Who has the mouse, by the engine's word; `None` without it.
    pub pointer: Option<Pointer>,
    /// A part of ours was pressed.
    pub pressed: bool,
    /// The keyboard we took is no longer ours: the person clicked the game, or another window took it.
    pub lost: bool,
    /// The person gave it back (Esc with a page up).
    pub given_back: bool,
}

/// Who has them from now on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Who {
    /// The game has both.
    #[default]
    Game,
    /// The mouse is the person's, over the game and ours alike; the game keeps the keyboard.
    Mouse,
    /// The parts have both.
    Parts,
}

#[derive(Debug, Default)]
pub struct Pointing {
    /// The person gave the game its mouse back with the key still held, or the page still up: until the key is pressed again, or a page opens, or a part is pressed.
    yielded: bool,
    was: Turn,
}

impl Pointing {
    pub fn turn(&mut self, now: Turn) -> Who {
        let was = std::mem::replace(&mut self.was, now);
        if !now.front || now.open.is_none() {
            self.yielded = false;
            return Who::Game;
        }
        if now.lost || now.given_back {
            self.yielded = true;
        }
        // The key pressed, a page opened, something to type in, or a part of ours pressed: asked anew.
        if (now.option && !was.option) || (now.sheet && !was.sheet) || (now.typing && !was.typing) || now.pressed {
            self.yielded = false;
        }
        // The mouse is the person's already: taking the keyboard is for what is typed alone.
        let free = matches!(now.pointer, Some(Pointer::Free | Pointer::Plain));
        let wants = if free {
            now.sheet && now.typing
        } else {
            // VRChat's wheel with the engine's word is picked by the mouse's own movement ([`super::gesture`]): the key is for the engine that gives none, and for a Quick
            // Menu it holds the mouse in all the same.
            let key = now.option && (now.pointer.is_none() || now.open != Some(Around::Wheel));
            // A page put away with the key still held: the key still says who has the mouse.
            key || now.sheet
        };
        match () {
            () if wants && !self.yielded => Who::Parts,
            () if free => Who::Mouse,
            () => Who::Game,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHEEL: Turn = Turn { front: true, open: Some(Around::Wheel), sheet: false, typing: false, option: false, pointer: None, pressed: false, lost: false, given_back: false };

    /// An engine from before, or one whose word of the mouse is not there: Left Option frees the mouse around both of VRChat's menus, and only while held.
    #[test]
    fn left_option_points_while_held_with_either_menu_up() {
        for open in [Around::Wheel, Around::QuickMenu] {
            let mut p = Pointing::default();
            let turn = Turn { open: Some(open), ..WHEEL };
            assert_eq!(p.turn(turn), Who::Game, "the game has it");
            assert_eq!(p.turn(Turn { option: true, ..turn }), Who::Parts);
            assert_eq!(p.turn(Turn { option: true, ..turn }), Who::Parts);
            assert_eq!(p.turn(turn), Who::Game, "let go: the game's at once");
        }
        let mut p = Pointing::default();
        assert_eq!(p.turn(Turn { open: None, option: true, ..WHEEL }), Who::Game, "no menu: Option is the game's");
        assert_eq!(p.turn(Turn { front: false, option: true, ..WHEEL }), Who::Game, "another app in front: Option is that app's");
    }

    /// A page keeps it with no key held, from the press that opened it until it closes.
    #[test]
    fn a_page_keeps_the_mouse_until_it_closes() {
        let mut p = Pointing::default();
        p.turn(WHEEL);
        assert_eq!(p.turn(Turn { option: true, ..WHEEL }), Who::Parts);
        assert_eq!(p.turn(Turn { option: true, sheet: true, ..WHEEL }), Who::Parts);
        assert_eq!(p.turn(Turn { sheet: true, ..WHEEL }), Who::Parts, "the key let go with the page up");
        assert_eq!(p.turn(WHEEL), Who::Game, "closed");
        // Opened by a number, or by N4N0: no key at all.
        assert_eq!(p.turn(Turn { sheet: true, ..WHEEL }), Who::Parts);
    }

    /// A click on the game gives it its mouse back with the page still up or the key still held; the key pressed again, or another page, asks anew.
    #[test]
    fn a_click_on_the_game_gives_it_the_mouse_back() {
        for pointer in [None, Some(Pointer::Held)] {
            let mut p = Pointing::default();
            let sheet = Turn { sheet: true, pointer, ..WHEEL };
            assert_eq!(p.turn(sheet), Who::Parts);
            assert_eq!(p.turn(Turn { lost: true, ..sheet }), Who::Game);
            assert_eq!(p.turn(sheet), Who::Game, "it stays the game's, the page up");
            assert_eq!(p.turn(Turn { option: true, ..sheet }), Who::Parts, "Left Option takes it again");
            assert_eq!(p.turn(sheet), Who::Parts);
            assert_eq!(p.turn(Turn { given_back: true, ..sheet }), Who::Game, "Esc");
            assert_eq!(p.turn(WHEEL), Who::Game);
            assert_eq!(p.turn(sheet), Who::Parts, "a page opened again");
        }
        // The key held through it: one press, one time.
        let mut p = Pointing::default();
        p.turn(WHEEL);
        let held = Turn { option: true, ..WHEEL };
        assert_eq!(p.turn(held), Who::Parts);
        assert_eq!(p.turn(Turn { lost: true, ..held }), Who::Game);
        assert_eq!(p.turn(held), Who::Game, "still held: it does not fight the person");
        assert_eq!(p.turn(WHEEL), Who::Game);
        assert_eq!(p.turn(held), Who::Parts, "pressed again");
    }

    /// Cmd-Tab away, the menu closed, the game ended: the keyboard is let go at once, and coming back starts clean.
    #[test]
    fn nothing_is_held_once_the_game_is_not_in_front() {
        for pointer in [None, Some(Pointer::Away), Some(Pointer::Free)] {
            for gone in [Turn { front: false, option: true, sheet: true, typing: true, pointer, ..WHEEL }, Turn { open: None, option: true, sheet: true, typing: true, pointer, ..WHEEL }] {
                let mut p = Pointing::default();
                let up = Turn { sheet: true, typing: true, pointer, ..WHEEL };
                p.turn(WHEEL);
                assert_eq!(p.turn(up), Who::Parts);
                assert_eq!(p.turn(gone), Who::Game);
                assert_eq!(p.turn(up), Who::Parts, "back, with the page still up");
            }
        }
        // A click on the game, then away and back: what was given back is forgotten with the app.
        let mut p = Pointing::default();
        p.turn(Turn { sheet: true, ..WHEEL });
        p.turn(Turn { sheet: true, lost: true, ..WHEEL });
        p.turn(Turn { front: false, ..WHEEL });
        assert_eq!(p.turn(Turn { sheet: true, ..WHEEL }), Who::Parts);
    }

    /// The engine lets the mouse go in the Quick Menu: the parts and the pages are clicked with no key, and the game keeps its keyboard until something of ours is typed in;
    /// given back the moment it is not, or the person clicks the game, and asked for anew by a press on ours.
    #[test]
    fn with_the_mouse_let_go_the_game_keeps_its_keyboard_until_one_types() {
        let free = Turn { open: Some(Around::QuickMenu), pointer: Some(Pointer::Free), ..WHEEL };
        let mut p = Pointing::default();
        assert_eq!(p.turn(free), Who::Mouse, "no key");
        assert_eq!(p.turn(Turn { option: true, ..free }), Who::Mouse, "Option is the game's");
        let page = Turn { sheet: true, ..free };
        assert_eq!(p.turn(page), Who::Mouse, "a page is clicked as a part is");
        let types = Turn { typing: true, ..page };
        assert_eq!(p.turn(types), Who::Parts, "a text field");
        // (with our window having the keyboard, the engine's word is the same: it confines nothing)
        assert_eq!(p.turn(types), Who::Parts);
        assert_eq!(p.turn(page), Who::Mouse, "left: the game's at once");
        assert_eq!(p.turn(types), Who::Parts);
        assert_eq!(p.turn(Turn { lost: true, ..types }), Who::Mouse, "a click on the game");
        assert_eq!(p.turn(types), Who::Mouse, "it stays the game's, the field up");
        assert_eq!(p.turn(Turn { pressed: true, ..types }), Who::Parts, "a press on ours");
        assert_eq!(p.turn(types), Who::Parts);
        assert_eq!(p.turn(free), Who::Mouse);
        // Nothing of ours to type in, whatever was said of it: no page, no keyboard.
        assert_eq!(p.turn(Turn { typing: true, ..free }), Who::Mouse);
        // A game that shows its own cursor leaves it as well.
        assert_eq!(p.turn(Turn { pointer: Some(Pointer::Plain), ..page }), Who::Mouse);
    }

    /// VRChat's wheel holds the mouse: the ring is picked by its movement, not by a key; a page opened from it takes the keyboard, which frees the mouse.
    #[test]
    fn around_the_wheel_only_a_page_takes_the_keyboard() {
        let held = Turn { pointer: Some(Pointer::Held), ..WHEEL };
        let mut p = Pointing::default();
        assert_eq!(p.turn(held), Who::Game);
        assert_eq!(p.turn(Turn { option: true, ..held }), Who::Game, "no key");
        assert_eq!(p.turn(Turn { sheet: true, ..held }), Who::Parts);
        assert_eq!(p.turn(Turn { sheet: true, pointer: Some(Pointer::Away), ..held }), Who::Parts, "the engine let it go");
        assert_eq!(p.turn(held), Who::Game);
        // A Quick Menu the engine holds the mouse in (VRChat did not let it go): the key.
        let odd = Turn { open: Some(Around::QuickMenu), ..held };
        assert_eq!(p.turn(odd), Who::Game);
        assert_eq!(p.turn(Turn { option: true, ..odd }), Who::Parts);
    }
}
