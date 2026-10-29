//! N4N0's screen mirror (contracts `Docs/orb.md`, Screen mirror, from screen version 14): the camera N4N0 leaves in the
//! world before a world's video screen. The Screen mirror page asks ([`MirrorAsk`]); this keeps where the camera is (its
//! aim's countdown, placed, its zoom) and sends `N4N0/Mirror/On`, `Placed` and `Zoom` as they change, to an avatar that
//! has them. The camera renders only while its page shows on the avatar's screen: off the page, it stays where it is,
//! and coming back shows it at once.

use orbiters_contracts::orb::mirror as contract;
use orbiters_orb::mirror::{MirrorAsk, MirrorView, Phase, Reach};
use orbiters_orb::text::Message;
use rosc::OscType;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub(crate) struct MirrorRuntime {
    placed: bool,
    /// Aiming: when the camera stays where its owner looks.
    aim_until: Option<Instant>,
    zoom: u8,
    /// What `On`, `Placed` and `Zoom` were last sent as, to this avatar.
    sent: [Option<f32>; 3],
}

impl Default for MirrorRuntime {
    fn default() -> Self {
        Self { placed: false, aim_until: None, zoom: contract::ZOOM_DEFAULT, sent: [None; 3] }
    }
}

const AIM: Duration = Duration::from_secs(contract::AIM_SECONDS as u64);

impl MirrorRuntime {
    pub(crate) fn ask(&mut self, ask: MirrorAsk, now: Instant) {
        match ask {
            MirrorAsk::Show if !self.placed && self.aim_until.is_none() => self.aim_until = Some(now + AIM),
            MirrorAsk::Show => {}
            MirrorAsk::Aim => {
                self.placed = false;
                self.aim_until = Some(now + AIM);
            }
            MirrorAsk::ZoomIn => self.zoom = (self.zoom + 1).min(contract::ZOOM_STEPS),
            MirrorAsk::ZoomOut => self.zoom = self.zoom.saturating_sub(1),
            MirrorAsk::Stop => {
                self.placed = false;
                self.aim_until = None;
            }
        }
    }

    /// Another avatar (or none): no camera of the last one is anywhere; the new one gets the state afresh.
    pub(crate) fn forget_avatar(&mut self) {
        self.placed = false;
        self.aim_until = None;
        self.sent = [None; 3];
    }

    /// The messages to send now to an avatar with the parameters `present`, the page showing on its screen (`shown`): an
    /// aim whose time is up places the camera.
    pub(crate) fn tick(&mut self, now: Instant, shown: bool, present: &HashSet<String>) -> Vec<Message> {
        if self.aim_until.is_some_and(|t| t <= now) {
            self.aim_until = None;
            self.placed = true;
        }
        let on = shown && (self.placed || self.aim_until.is_some());
        let values = [(contract::ON, on as u8 as f32), (contract::PLACED, self.placed as u8 as f32), (contract::ZOOM, self.zoom as f32 / contract::ZOOM_STEPS as f32)];
        let mut out = Vec::new();
        for (i, (name, value)) in values.into_iter().enumerate() {
            if !present.contains(name) || self.sent[i] == Some(value) {
                continue;
            }
            self.sent[i] = Some(value);
            let arg = if name == contract::ZOOM { OscType::Float(value) } else { OscType::Bool(value != 0.0) };
            out.push((crate::runtime::parameter(name), arg));
        }
        out
    }

    /// What the page and More's card show: `reach` says whether the mirror can show where the menu is.
    pub(crate) fn view(&self, now: Instant, reach: Reach) -> MirrorView {
        let phase = match self.aim_until {
            Some(t) => Phase::Aiming { seconds: t.saturating_duration_since(now).as_secs_f32().ceil().max(1.0) as u8 },
            None if self.placed => Phase::Placed,
            None => Phase::Off,
        };
        MirrorView { phase, zoom: self.zoom, reach }
    }

    /// While aiming: the countdown's next second (the page shows it), then its end.
    pub(crate) fn next_due(&self, now: Instant) -> Option<Instant> {
        let end = self.aim_until?;
        let left = end.saturating_duration_since(now);
        let whole = Duration::from_secs(left.as_secs());
        Some(if left > whole { end - whole } else { end })
    }
}

/// Where the mirror can show: on the screen when the menu shows there and the avatar's N4N0 has the mirror.
pub(crate) fn reach(screen: bool, present: &HashSet<String>) -> Reach {
    match (present.contains(contract::ON), screen) {
        (false, _) => Reach::NoMirror,
        (true, false) => Reach::Overlay,
        (true, true) => Reach::Screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> HashSet<String> {
        contract::ALL.iter().map(|s| s.to_string()).collect()
    }

    fn value(out: &[Message], name: &str) -> Option<OscType> {
        out.iter().find(|(a, _)| a.ends_with(name)).map(|(_, v)| v.clone())
    }

    #[test]
    fn opening_the_page_aims_then_the_camera_stays_and_renders_only_while_the_page_shows() {
        let t0 = Instant::now();
        let mut m = MirrorRuntime::default();
        let first = m.tick(t0, false, &all());
        assert_eq!(value(&first, contract::ON), Some(OscType::Bool(false)));
        assert_eq!(value(&first, contract::ZOOM), Some(OscType::Float(3.0 / 8.0)));
        m.ask(MirrorAsk::Show, t0);
        let aiming = m.tick(t0, true, &all());
        assert_eq!(aiming, vec![(crate::runtime::parameter(contract::ON), OscType::Bool(true))], "rendering, following the eyes");
        assert_eq!(m.view(t0, Reach::Screen).phase, Phase::Aiming { seconds: 3 });
        assert_eq!(m.view(t0 + Duration::from_millis(1200), Reach::Screen).phase, Phase::Aiming { seconds: 2 });
        assert_eq!(m.next_due(t0 + Duration::from_millis(1200)), Some(t0 + Duration::from_secs(2)));
        assert!(m.tick(t0 + Duration::from_secs(1), true, &all()).is_empty());
        let placed = m.tick(t0 + AIM, true, &all());
        assert_eq!(placed, vec![(crate::runtime::parameter(contract::PLACED), OscType::Bool(true))]);
        assert_eq!(m.view(t0 + AIM, Reach::Screen).phase, Phase::Placed);
        assert_eq!(m.next_due(t0 + AIM), None);
        // Off the page: no rendering, still placed; back on it, at once, without aiming again.
        assert_eq!(m.tick(t0 + AIM, false, &all()), vec![(crate::runtime::parameter(contract::ON), OscType::Bool(false))]);
        m.ask(MirrorAsk::Show, t0 + AIM * 2);
        assert_eq!(m.view(t0 + AIM * 2, Reach::Screen).phase, Phase::Placed);
        assert_eq!(m.tick(t0 + AIM * 2, true, &all()), vec![(crate::runtime::parameter(contract::ON), OscType::Bool(true))]);
        // Zoom, aim again, stop.
        m.ask(MirrorAsk::ZoomIn, t0);
        assert_eq!(value(&m.tick(t0 + AIM * 2, true, &all()), contract::ZOOM), Some(OscType::Float(0.5)));
        for _ in 0..20 {
            m.ask(MirrorAsk::ZoomIn, t0);
        }
        assert_eq!(m.view(t0, Reach::Screen).zoom, contract::ZOOM_STEPS);
        m.ask(MirrorAsk::Aim, t0 + AIM * 3);
        assert_eq!(value(&m.tick(t0 + AIM * 3, true, &all()), contract::PLACED), Some(OscType::Bool(false)), "following the eyes again");
        m.ask(MirrorAsk::Stop, t0 + AIM * 3);
        let stopped = m.tick(t0 + AIM * 4, true, &all());
        assert_eq!(value(&stopped, contract::ON), Some(OscType::Bool(false)));
        assert_eq!(m.view(t0, Reach::Screen).phase, Phase::Off);
    }

    #[test]
    fn only_an_avatar_with_the_mirror_gets_it_and_a_new_avatar_starts_afresh() {
        let t0 = Instant::now();
        let mut m = MirrorRuntime::default();
        m.ask(MirrorAsk::Show, t0);
        assert!(m.tick(t0, true, &HashSet::new()).is_empty());
        assert_eq!(reach(true, &HashSet::new()), Reach::NoMirror);
        assert_eq!(reach(false, &all()), Reach::Overlay);
        assert_eq!(reach(true, &all()), Reach::Screen);
        assert_eq!(m.tick(t0, true, &all()).len(), 3);
        m.forget_avatar();
        assert_eq!(m.view(t0, Reach::Screen).phase, Phase::Off);
        assert_eq!(m.tick(t0, false, &all()).len(), 3, "the new avatar gets every value");
    }
}
