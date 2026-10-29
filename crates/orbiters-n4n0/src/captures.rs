//! 3D captures shared from N4N0's menu (contracts `Docs/orb.md`, Menu, from screen version 12): the app lists the
//! captures and shares them ([`crate::Host::share_capture`], its Photos & captures' way), and hands N4N0 where they stand
//! ([`crate::N4n0::set_captures`]); N4N0 reads the VRChat friends a capture goes to while a menu's Share to … shows. Every
//! menu shows the same (the status' `captures` and `friends`, the screen's menu directly).

use crate::runtime::Shared;
use crate::status::NoticeKind;
use orbiters_orb::captures::{CapturesView, Friends, ShareAsk};

impl crate::N4n0 {
    /// Where the 3D captures stand now, as the app keeps them: the screen's menu shows them (and fetches the pictures it
    /// lacks), the other menus through the status.
    pub fn set_captures(self: &Shared, captures: CapturesView) {
        let effects = {
            let mut rt = self.orb.lock().unwrap();
            if rt.captures == captures {
                return;
            }
            rt.captures = captures.clone();
            let effects = rt.screen.menu.set_captures(captures);
            rt.wake();
            effects
        };
        self.screen_effects(effects);
        self.publish();
    }

    /// The VRChat friends a capture can go to, read again (a menu's Share to … opened: `on`), or forgotten (it closed).
    /// The friends known stay shown while they are read again.
    pub(crate) fn friends(self: &Shared, on: bool) {
        let number = {
            let mut rt = self.orb.lock().unwrap();
            rt.friends_asked = rt.friends_asked.wrapping_add(1);
            if !on {
                rt.friends = Friends::None;
            } else if !matches!(rt.friends, Friends::Ready { .. }) {
                rt.friends = Friends::Loading;
            }
            let friends = rt.friends.clone();
            rt.screen.menu.set_friends(friends);
            rt.wake();
            rt.friends_asked
        };
        self.publish();
        if !on {
            return;
        }
        let n = self.clone();
        std::thread::spawn(move || {
            let friends = match n.host.friends() {
                Ok(list) => Friends::ready(list),
                Err(message) => Friends::Failed { message },
            };
            {
                let mut rt = n.orb.lock().unwrap();
                // Closed or asked again meanwhile: this answer is not wanted.
                if rt.friends_asked != number {
                    return;
                }
                rt.friends = friends.clone();
                rt.screen.menu.set_friends(friends);
                rt.wake();
            }
            n.publish();
        });
    }

    /// Shares a 3D capture as a menu asked (on a thread: the app may read its files first); how it goes comes back with
    /// [`crate::N4n0::set_captures`].
    pub(crate) fn share_capture(self: &Shared, ask: ShareAsk) {
        let n = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = n.host.share_capture(ask) {
                n.notify(NoticeKind::Warning, e);
            }
        });
    }

    /// Lists the 3D captures again (a menu's History opened), on a thread.
    pub(crate) fn captures_list(self: &Shared) {
        let n = self.clone();
        std::thread::spawn(move || n.host.captures_list());
    }
}
