//! N4N0's screen mirror as the menus show it (contracts `Docs/n4n0.md`, Screen mirror, from screen version 14): a camera
//! N4N0 leaves in the world before a world's video screen, its picture on the Screen mirror page of N4N0's projected
//! screen. The menu asks ([`MirrorAsk`]); N4N0's runtime keeps the camera's state and hands back what the page shows
//! ([`MirrorView`]).

use serde::{Deserialize, Serialize};

/// What the Screen mirror page asks of N4N0's runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorAsk {
    /// The page opened: a mirror left in the world shows again, else the camera starts aiming. (The camera renders only
    /// while the page shows: the runtime reads that from the menu itself.)
    Show,
    /// The camera follows its owner's eyes for a few seconds, then stays where they looked.
    Aim,
    ZoomIn,
    ZoomOut,
    /// The mirror goes: the camera stops and comes back to its owner's eyes.
    Stop,
}

/// Where the camera is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum Phase {
    #[default]
    Off,
    /// Following its owner's eyes, this many seconds before it stays.
    Aiming { seconds: u8 },
    /// Staying where its owner looked.
    Placed,
}

/// Whether the mirror can show where the menu is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// The avatar's N4N0 has no mirror (an older one, or none).
    #[default]
    NoMirror,
    /// The menu shows in Orbiters - N4N0's overlay, which cannot show the avatar's camera.
    Overlay,
    /// The menu shows on N4N0's projected screen, whose N4N0 has the mirror.
    Screen,
}

/// The mirror as the Screen mirror page and More's card show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorView {
    pub phase: Phase,
    /// The zoom step, 0 (widest) to `orbiters_contracts::n4n0::mirror::ZOOM_STEPS`.
    pub zoom: u8,
    pub reach: Reach,
}

impl MirrorView {
    pub fn available(&self) -> bool {
        self.reach == Reach::Screen
    }

    pub fn can_zoom_in(&self) -> bool {
        self.zoom < orbiters_contracts::n4n0::mirror::ZOOM_STEPS
    }

    pub fn can_zoom_out(&self) -> bool {
        self.zoom > 0
    }

    /// The page's status line.
    pub fn state(&self) -> String {
        match self.phase {
            Phase::Off => "Off".into(),
            Phase::Aiming { seconds } => format!("Look at the world's screen · {seconds}"),
            Phase::Placed => format!("Mirroring · zoom {}×", self.magnification()),
        }
    }

    /// The page's note under its status.
    pub fn note(&self) -> &'static str {
        match self.phase {
            Phase::Off => "Aim, then look at the world's video screen: N4N0 leaves a small camera there.",
            Phase::Aiming { .. } => "The camera follows your eyes, then stays where you looked.",
            Phase::Placed => "Only you see the picture: others see this page. Aim again to move it.",
        }
    }

    /// More's card note when the mirror can't show (`None` when it can).
    pub fn unavailable_note(&self) -> Option<&'static str> {
        match self.reach {
            Reach::Screen => None,
            Reach::Overlay => Some("On N4N0's screen"),
            Reach::NoMirror => Some("Update N4N0"),
        }
    }

    /// How much the picture is magnified against the widest view, rounded for the status line.
    pub fn magnification(&self) -> String {
        use orbiters_contracts::n4n0::mirror::{fov, ZOOM_STEPS};
        let z = self.zoom.min(ZOOM_STEPS) as f64 / ZOOM_STEPS as f64;
        let k = (fov(0.0) / 2.0).to_radians().tan() / (fov(z) / 2.0).to_radians().tan();
        if k < 9.95 {
            format!("{k:.1}")
        } else {
            format!("{k:.0}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_says_where_the_camera_is_and_the_card_why_it_cannot_show() {
        let mut v = MirrorView { phase: Phase::Aiming { seconds: 3 }, zoom: 0, reach: Reach::Screen };
        assert_eq!(v.state(), "Look at the world's screen · 3");
        assert!(v.available() && v.unavailable_note().is_none());
        assert!(!v.can_zoom_out() && v.can_zoom_in());
        v.phase = Phase::Placed;
        assert_eq!(v.state(), "Mirroring · zoom 1.0×");
        v.zoom = orbiters_contracts::n4n0::mirror::ZOOM_STEPS;
        assert_eq!(v.state(), "Mirroring · zoom 10×");
        assert!(!v.can_zoom_in());
        v.reach = Reach::Overlay;
        assert_eq!(v.unavailable_note(), Some("On N4N0's screen"));
        // Every status and note fits its slot on the page.
        for phase in [Phase::Off, Phase::Aiming { seconds: 3 }, Phase::Placed] {
            let v = MirrorView { phase, zoom: 7, reach: Reach::Screen };
            assert!(v.state().chars().count() <= 46, "{}", v.state());
            assert!(v.note().chars().count() <= crate::menu::layout::MIRROR_NOTE_COLUMNS, "{}", v.note());
        }
    }
}
