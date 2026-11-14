//! SteamVR's overlay slots. There are 128 for every process together (openvr.h, `k_unMaxOverlayCount`): Enzo's own overlay apps
//! (OVR Toolkit, OVR Advanced Settings, SpaceCalibrator, Standable, VRChat's loading screens...) take many of them, and what Orbiters
//! holds comes out of the rest. On 2026-11-14 (0.3.5-dev.52) his 3D view got the last eight slots for its twelve faces, SteamVR refused the
//! ninth (`LIMIT_EXCEEDED`), the eight made stayed with no one holding them, and every press after that was refused with `KEY_IN_USE` (17):
//! "a 3D world overlay: SteamVR overlay error 17" (vrcompositor.txt destroyed `cc.orbiters.desktop.world.0.0` to `.1.1` only when the
//! process ended). So: a set of overlays is made whole or not at all, a key this process left behind is taken back, and what has no room
//! says so, so that room can be made (`OrbitVr::give_back_hidden`) and the making tried again.

use orbiters_openvr as vr;

/// `VROverlayError_OverlayLimitExceeded`: every slot of SteamVR's is taken.
pub const LIMIT_EXCEEDED: i32 = 13;
/// `VROverlayError_KeyInUse`: an overlay with that key exists, in this process or another.
pub const KEY_IN_USE: i32 = 17;

/// What making and letting go of overlays needs of SteamVR: [`vr::Overlay`], or a stand-in in tests.
pub trait Slots {
    fn create(&self, key: &str, name: &str) -> Result<vr::OverlayHandle, i32>;
    fn find(&self, key: &str) -> Result<vr::OverlayHandle, i32>;
    fn destroy(&self, handle: vr::OverlayHandle) -> Result<(), i32>;
}

impl Slots for vr::Overlay<'_> {
    fn create(&self, key: &str, name: &str) -> Result<vr::OverlayHandle, i32> {
        vr::Overlay::create(self, key, name)
    }
    fn find(&self, key: &str) -> Result<vr::OverlayHandle, i32> {
        vr::Overlay::find(self, key)
    }
    fn destroy(&self, handle: vr::OverlayHandle) -> Result<(), i32> {
        vr::Overlay::destroy(self, handle)
    }
}

/// A line in `vr-test.log`; none from tests, which must not write the person's own logs.
fn note(text: &str) {
    if !cfg!(test) {
        super::vr_log::line(text);
    }
}

/// `key`'s overlay, made. One this process left under that key is let go first and made anew; one of another process's is not
/// touched (SteamVR does not let it go), and its `KEY_IN_USE` is the answer.
pub fn create(slots: &dyn Slots, key: &str, name: &str) -> Result<vr::OverlayHandle, i32> {
    match slots.create(key, name) {
        Err(KEY_IN_USE) => {
            let left = slots.find(key).map_err(|_| KEY_IN_USE)?;
            slots.destroy(left).map_err(|_| KEY_IN_USE)?;
            note(&format!("overlay {key}: was left in SteamVR, taken back and made anew")); // i18n: not shown (a log's line)
            slots.create(key, name)
        }
        made => made,
    }
}

/// Every one of `keys`' overlays, or none: what was made of a set that cannot be made whole is let go again, so that it neither holds
/// slots nor keys for nothing.
pub fn create_all(slots: &dyn Slots, keys: &[String], name: &str) -> Result<Vec<vr::OverlayHandle>, i32> {
    let mut made = Vec::with_capacity(keys.len());
    for key in keys {
        match create(slots, key, name) {
            Ok(h) => made.push(h),
            Err(code) => {
                for h in made {
                    let _ = slots.destroy(h);
                }
                note(&format!("overlays {}..: SteamVR error {code} at {key}, none kept", keys.first().map_or("", String::as_str))); // i18n: not shown (a log's line)
                return Err(code);
            }
        }
    }
    Ok(made)
}

#[cfg(test)]
pub mod fake {
    //! SteamVR's overlay slots as a stand-in: 128 for all processes, keys unique among them, an overlay let go only by its own process.
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    pub const ALL: usize = 128;

    #[derive(Default)]
    pub struct World {
        /// Key → (owner, handle).
        pub overlays: HashMap<String, (u32, vr::OverlayHandle)>,
        next: vr::OverlayHandle,
    }

    impl World {
        pub fn free(&self) -> usize {
            ALL - self.overlays.len()
        }
    }

    /// One process's view of a shared [`World`].
    pub struct Process<'w> {
        pub world: &'w RefCell<World>,
        pub id: u32,
    }

    impl Process<'_> {
        /// Overlays of this process's, as SteamVR's compositor would list them.
        pub fn keys(&self) -> Vec<String> {
            let mut keys: Vec<String> = self.world.borrow().overlays.iter().filter(|(_, (o, _))| *o == self.id).map(|(k, _)| k.clone()).collect();
            keys.sort();
            keys
        }
        /// Takes `n` slots as other apps would.
        pub fn take(&self, n: usize) {
            for i in 0..n {
                let _ = Slots::create(self, &format!("other.{}.{i}", self.id), "other");
            }
        }
    }

    impl Slots for Process<'_> {
        fn create(&self, key: &str, _name: &str) -> Result<vr::OverlayHandle, i32> {
            let mut w = self.world.borrow_mut();
            if w.overlays.contains_key(key) {
                return Err(KEY_IN_USE);
            }
            if w.overlays.len() >= ALL {
                return Err(LIMIT_EXCEEDED);
            }
            w.next += 1;
            let h = w.next;
            w.overlays.insert(key.to_string(), (self.id, h));
            Ok(h)
        }
        fn find(&self, key: &str) -> Result<vr::OverlayHandle, i32> {
            self.world.borrow().overlays.get(key).map(|(_, h)| *h).ok_or(10)
        }
        fn destroy(&self, handle: vr::OverlayHandle) -> Result<(), i32> {
            let mut w = self.world.borrow_mut();
            let Some(key) = w.overlays.iter().find(|(_, (_, h))| *h == handle).map(|(k, (o, _))| (k.clone(), *o)) else { return Err(11) };
            if key.1 != self.id {
                return Err(12);
            }
            w.overlays.remove(&key.0);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use std::cell::RefCell;

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("cc.test.{i}")).collect()
    }

    #[test]
    fn a_set_with_no_room_keeps_nothing_and_comes_whole_once_there_is_room() {
        let world = RefCell::new(World::default());
        let (others, us) = (Process { world: &world, id: 1 }, Process { world: &world, id: 2 });
        others.take(ALL - 8);
        assert_eq!(create_all(&us, &keys(12), "x"), Err(LIMIT_EXCEEDED));
        assert!(us.keys().is_empty(), "nothing of a set that could not be made is left: {:?}", us.keys());
        assert_eq!(world.borrow().free(), 8);
        // The same again: still no room, still nothing kept, and never "key in use".
        assert_eq!(create_all(&us, &keys(12), "x"), Err(LIMIT_EXCEEDED));
        // Room made: it comes whole.
        for i in 0..4 {
            let h = others.find(&format!("other.1.{i}")).unwrap();
            others.destroy(h).unwrap();
        }
        assert_eq!(create_all(&us, &keys(12), "x").map(|h| h.len()), Ok(12));
        assert_eq!(world.borrow().free(), 0);
    }

    #[test]
    fn a_key_this_process_left_is_taken_back_and_another_process_s_is_not() {
        let world = RefCell::new(World::default());
        let (others, us) = (Process { world: &world, id: 1 }, Process { world: &world, id: 2 });
        // Enzo's state: eight of twelve left in SteamVR by a set that failed half way.
        for k in keys(8) {
            Slots::create(&us, &k, "x").unwrap();
        }
        let made = create_all(&us, &keys(12), "x").expect("its own leftovers are taken back");
        assert_eq!(made.len(), 12);
        assert_eq!(us.keys().len(), 12, "no second copy, nothing more held");
        // Another process's key is not ours to take.
        Slots::create(&others, "cc.theirs", "x").unwrap();
        assert_eq!(create(&us, "cc.theirs", "x"), Err(KEY_IN_USE));
        assert_eq!(others.keys(), vec!["cc.theirs".to_string()]);
    }
}
