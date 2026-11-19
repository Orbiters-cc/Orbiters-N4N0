//! What of a surface's picture SteamVR has taken, for every surface of the overlay that uploads a picture and takes input from its own
//! layout (N4N0's menu, the floating panel and its bar, band, strip and watch, the worlds pill and bar, the small view's X, the dashboard
//! tab) and for those that only show one (the coverage panel, the calibration views, the small-view hint).
//!
//! The rule (verify2): the pixels that show and the controls that are tested are of one generation. A picture SteamVR did not take keeps
//! its region, which is handed over again (with whatever changed since) until it is taken; until then the surface takes no pointing and
//! no press, only a release (a let-go is never lost: nothing stays held). The Quick Menu's tiles and the sheet follow the same rule with
//! their own bookkeeping (`orbit::Panel::not_presented`, `vr::sheet_unshown_after`).

use orbiters_openvr as vr;
use std::time::{Duration, Instant};

/// How long a refused picture waits before it is handed over again when nothing new changed.
pub(crate) const RETRY: Duration = Duration::from_millis(250);

/// One surface's (one overlay handle's) picture as SteamVR has it.
#[derive(Debug, Default)]
pub(crate) struct Presented {
    /// The region SteamVR did not take (to hand over again), and not before when.
    pending: Option<[u32; 4]>,
    retry_at: Option<Instant>,
}

impl Presented {
    /// Hands over `changed` together with what was refused before; `show` hands a region over and says whether SteamVR took it.
    /// With nothing changed, a refused region is handed over again, at most every [`RETRY`]. Returns whether the picture that
    /// shows now is of the current layout.
    pub(crate) fn hand_over(&mut self, changed: Option<[u32; 4]>, now: Instant, show: impl FnOnce([u32; 4]) -> bool) -> bool {
        let rect = match (changed, self.pending) {
            (None, None) => return true,
            (None, Some(_)) if self.retry_at.is_some_and(|at| now < at) => return false,
            (None, Some(pending)) => pending,
            (Some(changed), None) => changed,
            (Some(changed), Some(pending)) => union(changed, pending),
        };
        if show(rect) {
            self.pending = None;
            self.retry_at = None;
            true
        } else {
            self.pending = Some(rect);
            self.retry_at = Some(now + RETRY);
            false
        }
    }

    /// The picture that shows is of the current layout: the surface may be pointed at and pressed.
    pub(crate) fn current(&self) -> bool {
        self.pending.is_none()
    }

    /// Whether a SteamVR mouse event (its type) reaches the surface: all of them while its picture is current; while not, only a
    /// button's release.
    pub(crate) fn admits(&self, event_type: u32) -> bool {
        self.current() || event_type == vr::event::MOUSE_BUTTON_UP
    }

    /// The same for the egui events the floating panel and the dashboard tab are given.
    pub(crate) fn admits_egui(&self, event: &egui::Event) -> bool {
        self.current() || matches!(event, egui::Event::PointerButton { pressed: false, .. } | egui::Event::PointerGone)
    }

    /// A press decided from the surface's place (a laser, a fingertip): taken only while its picture is current.
    pub(crate) fn press(&self, pressed: bool) -> bool {
        pressed && self.current()
    }

    /// The surface is let go (destroyed, or to be painted whole when it shows again): nothing is owed.
    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }
}

fn union(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every surface that shows a picture (the coverage panel, the calibration views, the small-view hint, and all the others): a
    /// refused region is kept, joined with what changes next, and handed over again, at most every RETRY when nothing changed.
    #[test]
    fn a_refused_picture_is_kept_and_handed_over_again() {
        let t = Instant::now();
        let mut p = Presented::default();
        assert!(p.hand_over(None, t, |_| panic!("nothing to hand over")));
        assert!(!p.hand_over(Some([10, 10, 20, 20]), t, |_| false));
        assert!(!p.current());
        assert!(!p.hand_over(None, t + Duration::from_millis(100), |_| panic!("not before RETRY")));
        let mut asked = None;
        assert!(!p.hand_over(Some([0, 15, 5, 30]), t + Duration::from_millis(100), |r| {
            asked = Some(r);
            false
        }));
        assert_eq!(asked, Some([0, 10, 20, 30]), "what changed now and what was refused");
        let mut asked = None;
        assert!(p.hand_over(None, t + Duration::from_millis(400), |r| {
            asked = Some(r);
            true
        }));
        assert_eq!(asked, Some([0, 10, 20, 30]));
        assert!(p.current());
        assert!(p.hand_over(None, t + Duration::from_secs(1), |_| panic!("nothing owed")));
    }

    /// Surfaces given SteamVR's mouse events (N4N0's menu, the worlds pill and bar, the dashboard tab): while their picture is not
    /// current only a release reaches them.
    #[test]
    fn a_surface_behind_its_picture_takes_only_releases_of_steamvr_s_mouse() {
        let mut p = Presented::default();
        assert!(p.admits(vr::event::MOUSE_MOVE) && p.admits(vr::event::MOUSE_BUTTON_DOWN));
        p.hand_over(Some([0, 0, 1, 1]), Instant::now(), |_| false);
        assert!(!p.admits(vr::event::MOUSE_MOVE), "no pointing at an old picture");
        assert!(!p.admits(vr::event::MOUSE_BUTTON_DOWN), "no press on it");
        assert!(p.admits(vr::event::MOUSE_BUTTON_UP), "a let-go is never lost");
        p.forget();
        assert!(p.admits(vr::event::MOUSE_BUTTON_DOWN));
    }

    /// Surfaces given egui events (the floating panel, the dashboard tab): the same, as egui events.
    #[test]
    fn a_surface_behind_its_picture_takes_only_releases_of_egui() {
        use egui::{pos2, Event, PointerButton};
        let mut p = Presented::default();
        p.hand_over(Some([0, 0, 1, 1]), Instant::now(), |_| false);
        let down = Event::PointerButton { pos: pos2(1.0, 1.0), button: PointerButton::Primary, pressed: true, modifiers: Default::default() };
        let up = Event::PointerButton { pos: pos2(1.0, 1.0), button: PointerButton::Primary, pressed: false, modifiers: Default::default() };
        assert!(!p.admits_egui(&Event::PointerMoved(pos2(1.0, 1.0))));
        assert!(!p.admits_egui(&down));
        assert!(p.admits_egui(&up) && p.admits_egui(&Event::PointerGone));
    }

    /// Surfaces pressed from their place (the small view's X): no press while the picture is not current.
    #[test]
    fn a_surface_behind_its_picture_is_not_pressed() {
        let mut p = Presented::default();
        assert!(p.press(true) && !p.press(false));
        p.hand_over(Some([0, 0, 1, 1]), Instant::now(), |_| false);
        assert!(!p.press(true));
    }
}
