//! What the desktop's panels are made of on every system: decided and drawn the same over VRChat's window on Windows ([`super::desktop`]) and on a Mac
//! ([`super::desktop_mac`]). No window, no key and no capture is here: those are each system's own.

use super::zones::{self, Around, Placed, Zone};
use super::{Panel, Panels};
use egui::Pos2;
use orbiters_i18n::mark;
use std::sync::Arc;

/// What the loop decides from VRChat's window and the keyboard's owner, each turn (REQUEST15 item 1): whether ours show at all, and whether VRChat has the keyboard.
/// Before, ours went the moment another program had the keyboard (`in_front` = VRChat in front): now they stay as long as VRChat's window is on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Presence {
    /// Ours show: VRChat's window is there, not minimised, not hidden, not in exclusive full screen.
    pub(crate) show: bool,
    /// The keyboard is VRChat's (or a window of ours has it): the keys of ours (Esc and R for its menus, 1 to 6 on the ring, Left Alt, Alt+F7 to F9) are read, the
    /// pointer's hook may take the mouse, and ours are topmost. Otherwise none of that: what is typed is for the program in front.
    pub(crate) keyboard: bool,
}

impl Presence {
    pub(crate) fn of(found: bool, on_screen: bool, exclusive: bool, front_is_game_or_ours: bool) -> Self {
        let show = found && on_screen && !exclusive;
        Self { show, keyboard: show && front_is_game_or_ours }
    }
}

/// Whether the scroll wheel picks a cell of the ring, with no key held (`desktop::scroll`). Off since dev.25: in dev.24 a scroll's click that opened a sheet froze the
/// panels once on the real game, and the cause is not proven yet. Left Alt and the number keys work as ever.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const SCROLL_TO_PICK: bool = false;

/// What the caption says with nothing pointed at, which someone who has seen it often enough no longer needs in full.
#[derive(Default)]
pub(super) struct Hints {
    /// How often Left Alt was used on the ring, and how often the Quick Menu opened, since this started.
    pointed: u32,
    openings: u32,
    was_open: Option<Around>,
    was_pointing: bool,
}

impl Hints {
    /// How many times the full words show.
    const FULL: u32 = 3;

    pub(super) fn follow(&mut self, open: Option<Around>, pointing: bool) {
        if open != self.was_open && open == Some(Around::QuickMenu) {
            self.openings += 1;
        }
        if pointing && !self.was_pointing {
            self.pointed += 1;
        }
        (self.was_open, self.was_pointing) = (open, pointing);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn now(&self, open: Option<Around>, pointing: bool) -> Option<(&'static str, &'static str)> {
        match open {
            // Now the ring takes the mouse: it said so, it says so.
            Some(Around::Wheel) if pointing && self.pointed <= Self::FULL => Some((mark("Click a tool"), "")),
            Some(Around::Wheel) if pointing => None,
            // The scroll wheel first, where it is on: it needs no key (round 12's judges: Left Alt was the friction of the desktop's wheel).
            Some(Around::Wheel) if SCROLL_TO_PICK && self.pointed < Self::FULL => Some((mark("Scroll to pick a tool, then click"), mark("Or hold Left Alt to point; keys 1 to 6"))),
            Some(Around::Wheel) if SCROLL_TO_PICK => Some((mark("Scroll to pick, or hold Left Alt"), "")),
            Some(Around::Wheel) if self.pointed < Self::FULL => Some((mark("Hold Left Alt, then click a tool"), mark("Or press its number, 1 to 6, from the top clockwise"))),
            Some(Around::Wheel) => Some((mark("Left Alt to click, or a number"), "")),
            // Says what this is, and that it is not VRChat's.
            Some(Around::QuickMenu) if self.openings <= Self::FULL => Some((mark("Your tools"), mark("Worlds in 3D, videos, instances and 3D pictures."))),
            _ => None,
        }
    }

    /// The same on a Mac. `keyless`: its engine lets the mouse go in the Quick Menu, and the ring round the wheel is picked by pushing past the wheel's edge, opened as it says;
    /// `None`: an engine that holds the mouse for as long as VRChat is in front, its Quick Menu included, where nobody can guess the key that frees it. Its ring has as many
    /// cells as VRChat's camera gives it ([`super::Panels::on_a_mac`]).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn now_on_a_mac(&self, open: Option<Around>, pointing: bool, keyless: Option<orbiters_core::settings::WheelPick>) -> Option<(&'static str, &'static str)> {
        use orbiters_core::settings::WheelPick;
        if let Some(pick) = keyless {
            let opens = match pick {
                WheelPick::Both => mark("Scroll one notch or rest on it to open it"),
                WheelPick::Scroll => mark("Scroll one notch to open it"),
                WheelPick::Rest => mark("Rest on it to open it"),
            };
            return match open {
                Some(Around::Wheel) if pointing && self.pointed <= Self::FULL => Some((opens, "")),
                Some(Around::Wheel) if pointing => None,
                Some(Around::Wheel) if self.pointed < Self::FULL => Some((mark("Push past the wheel's edge to a tool"), opens)),
                Some(Around::Wheel) => Some((mark("Push past the edge, or a number"), "")),
                Some(Around::QuickMenu) if self.openings <= Self::FULL => Some((mark("Your tools"), mark("Worlds in 3D, videos, instances and 3D pictures."))),
                _ => None,
            };
        }
        match open {
            Some(Around::Wheel | Around::QuickMenu) if pointing && self.pointed <= Self::FULL => Some((mark("Click a tool"), "")),
            Some(Around::Wheel | Around::QuickMenu) if pointing => None,
            Some(Around::Wheel) if self.pointed < Self::FULL => Some((mark("Hold Left Option, then click a tool"), mark("Or press its number, 1 to 5, from the top clockwise"))),
            Some(Around::Wheel) => Some((mark("Left Option to click, or a number"), "")),
            Some(Around::QuickMenu) if self.pointed < Self::FULL => Some((mark("Hold Left Option, then click a tool"), mark("Worlds in 3D, videos, instances and pictures."))),
            Some(Around::QuickMenu) => Some((mark("Left Option to click"), "")),
            _ => None,
        }
    }
}

/// Which panel the point (window pixels) is on: its index in [`Panels::all`] and where in it in its units. The takeover in front first.
pub(super) fn hit(panels: &Panels, p: Pos2) -> Option<(usize, Pos2)> {
    let all: Vec<&Panel> = panels.all().collect();
    (0..all.len()).rev().find_map(|i| {
        let panel = all[i];
        (panel.shown && panel.placed.contains(p)).then(|| {
            let uv = (p - panel.placed.rect.min) / panel.placed.rect.size();
            (i, panel.to_logical([uv.x, uv.y]))
        })
    })
}

/// Where the panel's window goes (its top left corner, in the pixels `origin` is in) and its alpha as its spring has them: it comes from the menu's side.
pub(super) fn spot(panel: &Panel, origin: Pos2) -> (Zone, [i32; 2], u8) {
    let Placed { rect, center, unit, .. } = panel.placed;
    let way = panel.zone.ring().map_or((rect.center() - center).normalized(), |ring| zones::polar(1.0, ring.middle()));
    let at = origin + rect.min.to_vec2() + way * panel.outward() * unit;
    (panel.zone, [at.x.round() as i32, at.y.round() as i32], (panel.appear.alpha() * 255.0) as u8)
}

/// The app's click, computed on a thread at the start, so that the first press already has it.
pub(super) struct Clicks {
    output: orbiters_sound::Output,
    click: Arc<std::sync::OnceLock<Arc<orbiters_sound::Sound>>>,
}

impl Clicks {
    pub(super) fn start() -> Self {
        let click = Arc::new(std::sync::OnceLock::new());
        let slot = click.clone();
        let _ = std::thread::Builder::new().name("orbit-click".into()).spawn(move || {
            use orbiters_sound::{render, sounds, RATE};
            let _ = slot.set(Arc::new(render(&sounds::ROCKER, RATE)));
        });
        Self { output: orbiters_sound::Output::new(), click }
    }

    pub(super) fn play(&self) {
        if let Some(s) = self.click.get() {
            self.output.play(s);
        }
    }
}

/// The pointer over ours on the desktop: VRChat's own shape (drawn here, not VRChat's picture), in the accent the person chose.
pub(crate) mod arrow {
    use egui::{pos2, Color32, Pos2};

    /// The arrow in a box of 24 units: its tip at the top left (the hotspot), its two wings, the notch between them at its back. Two triangles, as it is not
    /// convex.
    pub(crate) const TIP: Pos2 = pos2(2.0, 2.0);
    const WING_RIGHT: Pos2 = pos2(19.5, 9.0);
    const NOTCH: Pos2 = pos2(10.6, 10.6);
    const WING_DOWN: Pos2 = pos2(9.0, 19.5);

    /// The arrow in a square of `px` pixels at `scale` (1.0: the 24 units' box), RGBA premultiplied.
    pub(crate) fn picture(px: u32, scale: f32, colour: Color32) -> Vec<u8> {
        let mut canvas = crate::overlay::Canvas::new(px, px);
        canvas.paint(0.0, |p| {
            let at = |q: Pos2| pos2(q.x * scale, q.y * scale);
            let outline = vec![at(TIP), at(WING_RIGHT), at(NOTCH), at(WING_DOWN)];
            // The dark edge first, half of it outside the arrow, then the arrow over it.
            p.add(egui::Shape::closed_line(outline, egui::Stroke::new(2.4 * scale, Color32::from_black_alpha(200))));
            for tri in [[TIP, WING_RIGHT, NOTCH], [TIP, NOTCH, WING_DOWN]] {
                p.add(egui::Shape::convex_polygon(tri.iter().map(|q| at(*q)).collect(), colour, egui::Stroke::NONE));
            }
        });
        canvas.pixels().to_vec()
    }
}

