//! The window's offer to close to the tray while it has been minimised for long and VRChat runs (`crate::window_rest`):
//! the service only holds it in the snapshot, for the SteamVR overlay's band to show, and carries back what the band
//! said of it (shown, Keep open).
use super::{Controller, Shared};
use crate::state::RestOffer;

impl Controller {
    /// The window's offer, or none any more (withdrawn, the window gone). Another offer (or none) forgets what was said
    /// of the last: a next window numbers its offers from 1 again.
    pub fn window_rest(self: &Shared, offer: Option<RestOffer>) {
        self.update(|s| {
            let r = &mut s.window_rest;
            if offer.map(|o| o.id) != r.offer.map(|o| o.id) {
                r.shown = None;
                r.kept = None;
            }
            r.offer = offer;
        });
    }

    /// The offer the VR band shows now, or none (hidden, replaced, the overlay gone): a live fact the window's
    /// countdown runs on, never a latch (SWEEP-7).
    pub fn window_rest_shown(self: &Shared, id: Option<u64>) {
        self.update(|s| {
            s.window_rest.shown = id.filter(|id| s.window_rest.offer.is_some_and(|o| o.id == *id));
        });
    }

    /// Keep open, pressed in VR on the offer `id`: the window reads it and stays for the rest of the session.
    pub fn keep_window_open(self: &Shared, id: u64) {
        self.update(|s| {
            if s.window_rest.offer.is_some_and(|o| o.id == id) {
                s.window_rest.kept = Some(id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_band_says_belongs_to_the_offer_up() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        ctl.window_rest_shown(Some(1));
        ctl.keep_window_open(1);
        assert_eq!(ctl.view().window_rest, Default::default(), "nothing offered: nothing kept");
        ctl.window_rest(Some(RestOffer { id: 1, until_ms: 0, for_ms: 8_000, left_ms: 8_000 }));
        ctl.window_rest_shown(Some(1));
        assert_eq!(ctl.view().window_rest.shown, Some(1));
        // Night3, SWEEP-7: a live fact: the band hidden says so, and shows it again.
        ctl.window_rest_shown(None);
        assert_eq!(ctl.view().window_rest.shown, None, "hidden");
        ctl.window_rest_shown(Some(1));
        ctl.keep_window_open(2);
        assert_eq!(ctl.view().window_rest.kept, None, "another offer's press");
        ctl.keep_window_open(1);
        assert_eq!(ctl.view().window_rest.kept, Some(1));
        ctl.window_rest(Some(RestOffer { id: 1, until_ms: 9_000, for_ms: 8_000, left_ms: 8_000 }));
        assert_eq!(ctl.view().window_rest.kept, Some(1), "the same offer, its countdown started: still kept");
        ctl.window_rest(Some(RestOffer { id: 2, until_ms: 0, for_ms: 8_000, left_ms: 8_000 }));
        assert_eq!((ctl.view().window_rest.shown, ctl.view().window_rest.kept), (None, None), "a new offer: nothing said of it yet");
        ctl.window_rest(None);
        assert_eq!(ctl.view().window_rest.offer, None);
    }
}
