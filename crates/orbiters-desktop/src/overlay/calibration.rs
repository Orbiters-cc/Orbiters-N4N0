//! The T-pose calibration, in the play space. Pressing Calibrate in the dashboard tab fixes a frame in front of the
//! person (the headset's position and heading) and shows in the world: a mirror panel with the intended T-pose at
//! the person's size and the headset, controllers and trackers live on it, and a glowing target where each hand
//! goes. The countdown starts once the dashboard is closed; the pose then has to be held half a second. The overlay
//! process ticks it every loop: the tab is not drawn while the dashboard is closed.

use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Stroke};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_contracts::performance::tracking::{Device, DeviceClass, Sample};
use orbiters_core::pose::{by_role, calibration_of, TPoseCheck};
use orbiters_openvr::Matrix34;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

pub const COUNTDOWN_S: f64 = 3.0;
pub const HOLD_TIMEOUT_S: f64 = 12.0;
/// Waiting for the dashboard to close gives up after this.
const WAIT_TIMEOUT_S: f64 = 90.0;
/// The mirror and the targets stay this long after a result; the tab shows it a little longer.
const RESULT_S: f64 = 3.0;
const IDLE_AFTER_S: f64 = 6.0;
/// The pose has to be held this long.
const HOLD_S: f64 = 0.5;
/// Mirror: distance ahead, width (its height follows the picture's aspect).
const MIRROR_AHEAD_M: f32 = 1.8;
pub const MIRROR_WIDTH_M: f32 = 1.1;
pub const MIRROR_PX: [u32; 2] = [600, 720];
pub const TARGET_WIDTH_M: f32 = 0.16;
pub const TARGET_PX: u32 = 160;
/// A hand this close to its target is on it.
const ON_TARGET_M: f32 = 0.18;

#[derive(Clone, Debug)]
pub enum Phase {
    Idle,
    /// Waiting for the dashboard to close.
    Waiting {
        since: f64,
    },
    Countdown {
        until: f64,
    },
    /// `held` is when the pose started to count.
    Holding {
        since: f64,
        held: Option<f64>,
        check: TPoseCheck,
    },
    Done {
        at: f64,
        eye: f32,
        span: f32,
    },
    Failed {
        at: f64,
        reason: &'static str,
    },
}

/// The person's frame when the calibration started, in SteamVR's standing space (metres, +Y up, -Z ahead).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// On the floor under the headset.
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    /// Eye height.
    pub eye: f32,
}

impl Frame {
    /// Hands this far apart in the intended pose (a comfortable full stretch: arm span is about the body's height).
    pub fn span(&self) -> f32 {
        0.92 * self.eye
    }

    /// Hand height in the intended pose.
    pub fn shoulder(&self) -> f32 {
        self.eye - 0.22
    }

    fn at(&self, side: f32, up: f32, ahead: f32) -> [f32; 3] {
        let (o, r, f) = (self.origin, self.right, self.forward);
        [o[0] + r[0] * side + f[0] * ahead, up, o[2] + r[2] * side + f[2] * ahead]
    }

    /// Where the left and right hands go.
    pub fn targets(&self) -> [[f32; 3]; 2] {
        let half = self.span() / 2.0;
        [self.at(-half, self.shoulder(), 0.05), self.at(half, self.shoulder(), 0.05)]
    }

    /// A point of the play space on the mirror: across (to the person's right) and up, in metres.
    pub fn mirror_xy(&self, p: [f32; 3]) -> [f32; 2] {
        let d = [p[0] - self.origin[0], p[2] - self.origin[2]];
        [d[0] * self.right[0] + d[1] * self.right[2], p[1]]
    }

    /// The mirror's placement: ahead of the person, centred a little below the eyes, facing them.
    pub fn mirror_transform(&self) -> Matrix34 {
        let height = MIRROR_WIDTH_M * MIRROR_PX[1] as f32 / MIRROR_PX[0] as f32;
        facing(self.at(0.0, (self.eye - 0.42).max(height / 2.0 + 0.05), MIRROR_AHEAD_M), self.forward)
    }

    pub fn target_transform(&self, hand: usize) -> Matrix34 {
        facing(self.targets()[hand], self.forward)
    }

    /// From a headset sample (Unity conventions: SteamVR's space with Z mirrored).
    fn of_head(head: &Sample) -> Option<Self> {
        let p = vr(head.position);
        let [x, y, z, w] = head.rotation;
        // The headset's forward (+Z in Unity), then into SteamVR's space.
        let f = vr([2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)]);
        let len = (f[0] * f[0] + f[2] * f[2]).sqrt();
        if len < 1e-3 || p[1] < 0.5 {
            return None;
        }
        let forward = [f[0] / len, 0.0, f[2] / len];
        // Right = up × (−forward) in a right-handed space.
        let right = [-forward[2], 0.0, forward[0]];
        Some(Self { origin: [p[0], 0.0, p[2]], forward, right, eye: p[1] })
    }
}

/// SteamVR's space from Unity conventions (Z mirrored).
fn vr(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

/// An overlay at `center` whose front (+Z) faces back along `forward`, upright.
fn facing(center: [f32; 3], forward: [f32; 3]) -> Matrix34 {
    let z = [-forward[0], 0.0, -forward[2]];
    let y = [0.0, 1.0, 0.0];
    let x = [z[2], 0.0, -z[0]]; // y × z
    Matrix34 { m: [[x[0], y[0], z[0], center[0]], [x[1], y[1], z[1], center[1]], [x[2], y[2], z[2], center[2]]] }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Head,
    LeftHand,
    RightHand,
    Tracker,
}

/// A device as the mirror shows it.
#[derive(Clone, Copy, Debug)]
pub struct Live {
    pub kind: Kind,
    pub position: [f32; 3],
    pub ok: bool,
}

pub struct Calibrator {
    pub phase: Phase,
    pub frame: Option<Frame>,
    pub live: Vec<Live>,
}

impl Default for Calibrator {
    fn default() -> Self {
        Self { phase: Phase::Idle, frame: None, live: Vec::new() }
    }
}

impl Calibrator {
    /// A countdown, a pose being held, or waiting for the dashboard to close.
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Waiting { .. } | Phase::Countdown { .. } | Phase::Holding { .. })
    }

    /// Whether the mirror and the targets are in the world.
    pub fn in_world(&self, now: f64) -> bool {
        self.frame.is_some()
            && match self.phase {
                Phase::Done { at, .. } | Phase::Failed { at, .. } => now - at < RESULT_S,
                Phase::Idle => false,
                _ => true,
            }
    }

    /// Starts from the headset's current pose (the person faces the dashboard).
    pub fn start(&mut self, now: f64, devices: &[Device], samples: &[Sample]) {
        match by_role("head", devices, samples).and_then(Frame::of_head) {
            Some(frame) => {
                self.frame = Some(frame);
                self.phase = Phase::Waiting { since: now };
                self.update_live(devices, samples);
            }
            None => self.phase = Phase::Failed { at: now, reason: "The headset isn't tracked." },
        }
    }

    pub fn cancel(&mut self) {
        self.phase = Phase::Idle;
    }

    /// Advances the calibration; returns the calibration when a T-pose was held.
    pub fn tick(&mut self, now: f64, dashboard_visible: bool, devices: &[Device], samples: &[Sample]) -> Option<Calibration> {
        if self.in_world(now) {
            self.update_live(devices, samples);
        }
        match self.phase.clone() {
            Phase::Waiting { .. } if !dashboard_visible => self.phase = Phase::Countdown { until: now + COUNTDOWN_S },
            Phase::Waiting { since } if now - since > WAIT_TIMEOUT_S => self.phase = Phase::Idle,
            Phase::Countdown { until } if now >= until => self.phase = Phase::Holding { since: now, held: None, check: TPoseCheck::default() },
            Phase::Holding { since, held, .. } => {
                let check = match (by_role("head", devices, samples), by_role("left_hand", devices, samples), by_role("right_hand", devices, samples)) {
                    (Some(h), Some(l), Some(r)) => Some(TPoseCheck::of(h, l, r)),
                    _ => None,
                };
                match check {
                    None => self.phase = Phase::Failed { at: now, reason: "The headset and both controllers need to be tracked." },
                    Some(check) if check.all() => {
                        let from = held.unwrap_or(now);
                        if now - from >= HOLD_S {
                            let c = calibration_of(orbiters_core::clock::now_ns() as i64, "overlay", devices, samples);
                            self.phase = Phase::Done { at: now, eye: c.head_height_m, span: c.arm_span_m };
                            return Some(c);
                        }
                        self.phase = Phase::Holding { since, held: Some(from), check };
                    }
                    Some(_) if now - since > HOLD_TIMEOUT_S => {
                        self.phase = Phase::Failed { at: now, reason: "No T-pose seen: stand straight, hands on the two targets." };
                    }
                    Some(check) => self.phase = Phase::Holding { since, held: None, check },
                }
            }
            Phase::Done { at, .. } | Phase::Failed { at, .. } if now - at > IDLE_AFTER_S => self.phase = Phase::Idle,
            _ => {}
        }
        None
    }

    fn update_live(&mut self, devices: &[Device], samples: &[Sample]) {
        let Some(frame) = self.frame else { return };
        let targets = frame.targets();
        let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        self.live = samples
            .iter()
            .filter(|s| s.pose_valid())
            .filter_map(|s| {
                let d = devices.get(s.device as usize)?;
                let position = vr(s.position);
                let (kind, ok) = match (d.class, d.role.as_str()) {
                    (DeviceClass::Hmd, _) | (_, "head") => {
                        let off = ((position[0] - frame.origin[0]).powi(2) + (position[2] - frame.origin[2]).powi(2)).sqrt();
                        (Kind::Head, off < 0.25)
                    }
                    (_, "left_hand") => (Kind::LeftHand, dist(position, targets[0]) < ON_TARGET_M),
                    (_, "right_hand") => (Kind::RightHand, dist(position, targets[1]) < ON_TARGET_M),
                    _ => (Kind::Tracker, true),
                };
                Some(Live { kind, position, ok })
            })
            .collect();
    }

    /// Whether hand `i` (0 left, 1 right) is on its target.
    pub fn hand_on_target(&self, i: usize) -> bool {
        let kind = if i == 0 { Kind::LeftHand } else { Kind::RightHand };
        self.live.iter().any(|l| l.kind == kind && l.ok)
    }
}

/// The mirror panel, `MIRROR_PX` points.
pub fn paint_mirror(p: &Painter, cal: &Calibrator, now: f64) {
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(MIRROR_PX[0] as f32, MIRROR_PX[1] as f32));
    p.rect_filled(rect.shrink(2.0), theme::radius(28.0), theme::alpha(theme::INK_900, 0.94));
    p.rect_stroke(rect.shrink(2.0), theme::radius(28.0), Stroke::new(2.0, theme::white(0.10)), egui::StrokeKind::Inside);
    let Some(frame) = cal.frame else { return };

    // Heading.
    let (title, line, tone): (String, String, Color32) = match &cal.phase {
        Phase::Waiting { .. } => ("Close the dashboard".into(), "Then face this mirror and stand straight.".into(), theme::FG),
        Phase::Countdown { until } => (format!("{}", (until - now).ceil().max(1.0) as u32), "Put your hands on the two targets.".into(), theme::FG),
        Phase::Holding { check, held, .. } if check.all() => {
            let k = held.map_or(0.0, |h| ((now - h) / HOLD_S).clamp(0.0, 1.0));
            (if k >= 1.0 { "Got it".into() } else { "Hold it…".into() }, "Stay still for a moment.".into(), theme::OK)
        }
        Phase::Holding { check, .. } => ("T-pose".into(), first_fix(check).into(), theme::WARN),
        Phase::Done { eye, span, .. } => ("Calibrated".into(), format!("Eyes at {eye:.2} m, hands {span:.2} m apart."), theme::OK),
        Phase::Failed { reason, .. } => ("Not calibrated".into(), (*reason).into(), theme::DANGER),
        Phase::Idle => (String::new(), String::new(), theme::FG),
    };
    widgets::text(p, pos2(rect.center().x, 62.0), Align2::CENTER_CENTER, &Txt::new(title, 44.0).weight(weight::SEMIBOLD).color(tone));
    widgets::text(p, pos2(rect.center().x, 108.0), Align2::CENTER_CENTER, &Txt::new(line, 19.0).color(theme::FG_2));
    if let Phase::Holding { check, held: Some(h), .. } = &cal.phase {
        if check.all() {
            let k = ((now - h) / HOLD_S).clamp(0.0, 1.0) as f32;
            let bar = Rect::from_center_size(pos2(rect.center().x, 132.0), vec2(220.0, 6.0));
            p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * k, bar.height())), theme::radius(3.0), theme::OK);
        }
    }

    // The diagram: the person seen in a mirror (their right on the right), the floor at the bottom.
    let area = Rect::from_min_max(pos2(30.0, 150.0), pos2(rect.right() - 30.0, 612.0));
    let span = frame.span();
    let s = (area.height() / (frame.eye + 0.25)).min(area.width() / (span + 0.5));
    let to = |x: f32, y: f32| pos2(area.center().x + x * s, area.bottom() - y * s);
    p.line_segment([pos2(area.left(), area.bottom()), pos2(area.right(), area.bottom())], Stroke::new(2.0, theme::white(0.08)));
    let ghost = Stroke::new(0.06 * s, theme::white(0.13));
    let (eye, sh) = (frame.eye, frame.shoulder());
    let hip = 0.53 * eye;
    p.circle_stroke(to(0.0, eye + 0.03), 0.1 * s, Stroke::new(0.04 * s, theme::white(0.13)));
    // Continuous lines: translucent strokes that overlap would show seams at the joints.
    for line in [
        vec![(0.0, eye - 0.08), (0.0, sh)],
        vec![(-span / 2.0, sh), (span / 2.0, sh)],
        vec![(0.0, sh), (0.0, hip)],
        vec![(-0.13, 0.04), (0.0, hip), (0.13, 0.04)],
    ] {
        p.add(egui::Shape::line(line.into_iter().map(|(x, y)| to(x, y)).collect(), ghost));
    }
    // Hand targets, pulsing until a hand is on them.
    let pulse = 0.5 + 0.5 * ((now * 4.0).sin() as f32);
    for (i, x) in [-span / 2.0, span / 2.0].into_iter().enumerate() {
        let on = cal.hand_on_target(i);
        let c = if on { theme::OK } else { theme::VIOLET };
        let r = ON_TARGET_M * s * if on { 1.0 } else { 0.9 + 0.1 * pulse };
        p.circle_filled(to(x, sh), r, theme::alpha(c, if on { 0.22 } else { 0.10 + 0.08 * pulse }));
        p.circle_stroke(to(x, sh), r, Stroke::new(3.0, theme::alpha(c, 0.9)));
    }
    // The devices, live.
    for l in &cal.live {
        let [x, y] = frame.mirror_xy(l.position);
        let c = match (l.kind, l.ok) {
            (Kind::Tracker, _) => theme::FG_2,
            (_, true) => theme::OK,
            (_, false) => theme::WARN,
        };
        let at = to(x, y);
        match l.kind {
            Kind::Head => {
                p.rect_filled(Rect::from_center_size(at, vec2(0.2 * s, 0.11 * s)), theme::radius(0.04 * s), c);
            }
            Kind::LeftHand | Kind::RightHand => {
                let i = if l.kind == Kind::LeftHand { 0 } else { 1 };
                if !l.ok {
                    let [tx, ty] = frame.mirror_xy(frame.targets()[i]);
                    p.line_segment([at, to(tx, ty)], Stroke::new(2.0, theme::alpha(theme::WARN, 0.6)));
                }
                p.circle_filled(at, 0.05 * s, c);
                widgets::text(p, at, Align2::CENTER_CENTER, &Txt::new(if i == 0 { "L" } else { "R" }, 0.05 * s).weight(weight::SEMIBOLD).color(theme::INK_950));
            }
            Kind::Tracker => {
                p.rect_filled(Rect::from_center_size(at, vec2(0.06 * s, 0.06 * s)), theme::radius(0.015 * s), c);
            }
        }
    }

    // The checks.
    let all = TPoseCheck { level: true, shoulders: true, wide: true, centered: true, still: true };
    let check = match &cal.phase {
        Phase::Holding { check, .. } => Some(check),
        Phase::Done { .. } => Some(&all),
        _ => None,
    };
    let items = [
        ("Arms out", check.map(|c| c.wide)),
        ("Level", check.map(|c| c.level)),
        ("Shoulders", check.map(|c| c.shoulders)),
        ("Centred", check.map(|c| c.centered)),
        ("Still", check.map(|c| c.still)),
    ];
    let w = (rect.width() - 40.0) / items.len() as f32;
    for (i, (label, ok)) in items.iter().enumerate() {
        let c = pos2(20.0 + w * (i as f32 + 0.5), 650.0);
        let color = match ok {
            Some(true) => theme::OK,
            Some(false) => theme::WARN,
            None => theme::FG_4,
        };
        widgets::icon(p, c, if *ok == Some(true) { icons::CHECK_CIRCLE } else { icons::CIRCLE }, 24.0, color);
        widgets::text(p, pos2(c.x, c.y + 30.0), Align2::CENTER_CENTER, &Txt::new(*label, 15.0).color(if *ok == Some(true) { theme::FG } else { theme::FG_3 }));
    }
}

/// What to fix first when the pose is not there yet.
fn first_fix(c: &TPoseCheck) -> &'static str {
    if !c.wide {
        "Stretch your arms out to the targets."
    } else if !c.shoulders {
        "Hands at shoulder height."
    } else if !c.level {
        "Keep both hands at the same height."
    } else if !c.centered {
        "Stand between your hands."
    } else {
        "Stay still."
    }
}

/// A hand target, `TARGET_PX` points: a ring that pulses until the hand is on it.
pub fn paint_target(p: &Painter, label: &str, on: bool, now: f64) {
    let c = pos2(TARGET_PX as f32 / 2.0, TARGET_PX as f32 / 2.0);
    let pulse = 0.5 + 0.5 * ((now * 4.0).sin() as f32);
    let color = if on { theme::OK } else { theme::VIOLET };
    let r = if on { 70.0 } else { 62.0 + 8.0 * pulse };
    p.circle_filled(c, r, theme::alpha(color, if on { 0.30 } else { 0.12 + 0.10 * pulse }));
    p.circle_stroke(c, r - 3.0, Stroke::new(6.0, color));
    widgets::text(p, c, Align2::CENTER_CENTER, &Txt::new(label, 44.0).weight(weight::SEMIBOLD).color(theme::FG));
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::performance::tracking::flags;

    fn body(left: [f32; 3], right: [f32; 3]) -> (Vec<Device>, Vec<Sample>) {
        let at = |device: u8, position: [f32; 3]| Sample { device, flags: flags::POSE_VALID, position, rotation: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        let device = |class, role: &str| Device { class, role: role.into(), ..Default::default() };
        (
            vec![device(DeviceClass::Hmd, "head"), device(DeviceClass::Controller, "left_hand"), device(DeviceClass::Controller, "right_hand")],
            vec![at(0, [0.0, 1.6, 0.0]), at(1, left), at(2, right)],
        )
    }

    #[test]
    fn mirror_and_targets_face_the_person() {
        // Facing Unity's +Z, which is SteamVR's −Z.
        let (d, s) = body([-0.7, 1.4, 0.0], [0.7, 1.4, 0.0]);
        let mut c = Calibrator::default();
        c.start(0.0, &d, &s);
        let f = c.frame.unwrap();
        assert!((f.forward[2] + 1.0).abs() < 1e-5 && (f.right[0] - 1.0).abs() < 1e-5);
        let m = f.mirror_transform().m;
        assert!((m[2][3] + MIRROR_AHEAD_M).abs() < 1e-4, "ahead: {m:?}");
        assert!((m[2][2] - 1.0).abs() < 1e-5, "its front faces back at the person");
        // The left target is on the person's left, at shoulder height.
        let [l, r] = f.targets();
        assert!(l[0] < 0.0 && r[0] > 0.0 && (l[1] - (1.6 - 0.22)).abs() < 1e-5);
        assert_eq!(f.mirror_xy(r)[0], f.span() / 2.0);
    }

    #[test]
    fn waits_for_the_dashboard_then_counts_down_and_holds() {
        let (d, s) = body([-0.72, 1.4, 0.05], [0.72, 1.4, 0.05]);
        let mut c = Calibrator::default();
        c.start(0.0, &d, &s);
        assert!(c.tick(1.0, true, &d, &s).is_none());
        assert!(matches!(c.phase, Phase::Waiting { .. }), "the dashboard is still open");
        c.tick(2.0, false, &d, &s);
        assert!(matches!(c.phase, Phase::Countdown { .. }));
        c.tick(2.0 + COUNTDOWN_S, false, &d, &s);
        assert!(matches!(c.phase, Phase::Holding { .. }));
        assert!(c.hand_on_target(0) && c.hand_on_target(1));
        assert!(c.tick(5.1, false, &d, &s).is_none());
        let done = c.tick(5.7, false, &d, &s).expect("held half a second");
        assert!((done.head_height_m - 1.6).abs() < 1e-5);
        assert!(c.in_world(5.7 + RESULT_S - 0.1) && !c.in_world(5.7 + RESULT_S + 0.1));
    }
}
