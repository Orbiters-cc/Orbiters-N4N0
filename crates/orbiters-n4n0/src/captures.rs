//! 3D captures shared from N4N0's menu (contracts `Docs/n4n0.md`, Menu, from screen version 12): the app lists the
//! captures and shares them ([`crate::Host::share_capture`], its Photos & captures' way), and hands N4N0 where they stand
//! ([`crate::N4n0::set_captures`]); N4N0 reads the VRChat friends a capture goes to while a menu's Share to … shows. Every
//! menu shows the same (the status' `captures` and `friends`, the screen's menu directly).

use crate::fence::Fence;
use crate::runtime::Shared;
use crate::status::NoticeKind;
use orbiters_n4n0_core::captures::{CapturesView, Friends, ShareAsk};

impl crate::N4n0 {
    /// Where the 3D captures stand now, as the app keeps them: the screen's menu shows them (and fetches the pictures it
    /// lacks), the other menus through the status.
    pub fn set_captures(self: &Shared, captures: CapturesView) {
        let effects = {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.captures == captures {
                return;
            }
            rt.captures = captures.clone().into();
            let effects = rt.screen.menu.set_captures(captures);
            rt.wake();
            effects
        };
        self.screen_effects(effects);
        // A share the assistant was asked for "the next one" goes once it is ready.
        self.share_next_check();
        self.publish();
    }

    /// The VRChat friends a capture can go to, read again (a menu's Share to … opened: `on`), or forgotten (it closed).
    /// The friends known stay shown while they are read again.
    /// Forgotten only once no menu (`holder`: the one asking) still shows Share to … (MENU-13).
    pub(crate) fn friends(self: &Shared, on: bool, holder: crate::MenuHolder) {
        let number = {
            let mut rt = self.n4n0.lock().unwrap();
            if on {
                rt.friends_holders.insert(holder);
            } else if holder == crate::MenuHolder::Any {
                rt.friends_holders.clear();
            } else {
                rt.friends_holders.remove(&holder);
            }
            if !on && !rt.friends_holders.is_empty() {
                // Another menu's Share to … still lists them.
                return;
            }
            rt.friends_asked = rt.friends_asked.wrapping_add(1);
            if !on {
                rt.friends = Friends::None.into();
            } else if !matches!(*rt.friends, Friends::Ready { .. }) {
                rt.friends = Friends::Loading.into();
            }
            let friends = (*rt.friends).clone();
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
                let mut rt = n.n4n0.lock().unwrap();
                // Closed or asked again meanwhile: this answer is not wanted.
                if rt.friends_asked != number {
                    return;
                }
                rt.friends = friends.clone().into();
                rt.screen.menu.set_friends(friends);
                rt.wake();
            }
            n.publish();
        });
    }

    /// Lists the 3D captures again (a menu's History opened), on a thread.
    pub(crate) fn captures_list(self: &Shared) {
        let n = self.clone();
        std::thread::spawn(move || n.host.captures_list());
    }
}

impl Fence<'_> {
    /// Shares a 3D capture as a menu asked (on a thread: the app may read its files first); how it goes comes back with
    /// [`crate::N4n0::set_captures`].
    pub(crate) fn share_capture(&self, ask: ShareAsk) {
        let later = self.later();
        std::thread::spawn(move || {
            let Some(fence) = later.fence() else { return };
            if let Err(e) = fence.host.share_capture(ask) {
                fence.notify(NoticeKind::Warning, e);
            }
        });
    }
}
