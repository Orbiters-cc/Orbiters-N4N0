//! What the SteamVR dashboard tab shows: the recorder, what each stream records right now, and the T-pose
//! calibration. Laid out for a fixed 1024 × 640 point canvas, read from a metre or two away.

use crate::backend::Ctl;
use crate::state::{CaptureState, Snapshot};
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Ui, UiBuilder, Vec2};
use orbiters_contracts::performance::tracking::{Device, Sample};
use orbiters_core::pose::{by_role, calibration_of, TPoseCheck};
use orbiters_core::settings::Settings;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{format, icons, logo};

pub const SIZE: Vec2 = Vec2::new(1024.0, 640.0);

/// Device poses for calibrating (SteamVR in the overlay, a stand-in in screenshots).
pub trait Poses {
    /// Connected devices, numbered by position, and a sample of each (play space, Unity conventions).
    fn read(&mut self) -> (Vec<Device>, Vec<Sample>);
}

#[derive(Clone, Debug)]
enum Calibrating {
    Idle,
    Countdown {
        until: f64,
    },
    /// `held` is when the pose started to count (it must stay half a second).
    Holding {
        since: f64,
        held: Option<f64>,
        check: TPoseCheck,
    },
    Done {
        at: f64,
    },
    Failed {
        at: f64,
        reason: &'static str,
    },
}

const COUNTDOWN_S: f64 = 3.0;
const HOLD_TIMEOUT_S: f64 = 12.0;

pub struct OverlayUi {
    calibrating: Calibrating,
}

impl Default for OverlayUi {
    fn default() -> Self {
        Self { calibrating: Calibrating::Idle }
    }
}

impl OverlayUi {
    /// Whether frames must keep coming (a countdown, a T-pose being held).
    pub fn busy(&self) -> bool {
        matches!(self.calibrating, Calibrating::Countdown { .. } | Calibrating::Holding { .. })
    }

    /// Starts a calibration as if the button was pressed (screenshots).
    pub fn start_calibration(&mut self, now: f64) {
        self.calibrating = Calibrating::Countdown { until: now + COUNTDOWN_S };
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, poses: &mut dyn Poses) {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
        header(ui, snap, rect);
        let left = Rect::from_min_size(pos2(24.0, 96.0), vec2(452.0, 520.0));
        let right = Rect::from_min_max(pos2(left.right() + 20.0, left.top()), pos2(rect.right() - 24.0, left.bottom()));
        recorder(ui, snap, ctl, left);
        let streams_rect = Rect::from_min_size(right.min, vec2(right.width(), 318.0));
        streams(ui, snap, ctl, streams_rect);
        let calib_rect = Rect::from_min_max(pos2(right.left(), streams_rect.bottom() + 16.0), right.max);
        self.calibration(ui, snap, ctl, poses, calib_rect);
    }

    fn calibration(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, poses: &mut dyn Poses, rect: Rect) {
        let now = ui.input(|i| i.time);
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(rect.left() + 22.0, rect.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("T-pose calibration"));

        // Advance the state machine.
        match self.calibrating.clone() {
            Calibrating::Countdown { until } if now >= until => {
                self.calibrating = Calibrating::Holding { since: now, held: None, check: TPoseCheck::default() };
            }
            Calibrating::Holding { since, mut held, .. } => {
                let (devices, samples) = poses.read();
                let check = match (by_role("head", &devices, &samples), by_role("left_hand", &devices, &samples), by_role("right_hand", &devices, &samples)) {
                    (Some(h), Some(l), Some(r)) => Some(TPoseCheck::of(h, l, r)),
                    _ => None,
                };
                match check {
                    None => self.calibrating = Calibrating::Failed { at: now, reason: "The headset and both controllers need to be tracked." },
                    Some(check) => {
                        if check.all() {
                            let from = *held.get_or_insert(now);
                            if now - from >= 0.5 {
                                ctl.calibrated(calibration_of(orbiters_core::clock::now_ns() as i64, "overlay", &devices, &samples));
                                self.calibrating = Calibrating::Done { at: now };
                            } else {
                                self.calibrating = Calibrating::Holding { since, held, check };
                            }
                        } else if now - since > HOLD_TIMEOUT_S {
                            self.calibrating = Calibrating::Failed { at: now, reason: "No T-pose seen: stand straight, arms out at shoulder height." };
                        } else {
                            self.calibrating = Calibrating::Holding { since, held: None, check };
                        }
                    }
                }
            }
            Calibrating::Done { at } | Calibrating::Failed { at, .. } if now - at > 6.0 => self.calibrating = Calibrating::Idle,
            _ => {}
        }
        if self.busy() {
            ui.ctx().request_repaint();
        }

        let body = Rect::from_min_max(pos2(rect.left() + 22.0, rect.top() + 48.0), pos2(rect.right() - 22.0, rect.bottom() - 18.0));
        match &self.calibrating {
            Calibrating::Idle | Calibrating::Done { .. } | Calibrating::Failed { .. } => {
                let (line, tone) = match (&self.calibrating, &snap.steamvr.calibration) {
                    (Calibrating::Failed { reason, .. }, _) => (reason.to_string(), theme::DANGER),
                    (Calibrating::Done { .. }, Some(c)) => {
                        (format!("Calibrated: eyes at {:.2} m, hands {:.2} m apart.", c.head_height_m, c.arm_span_m), theme::OK)
                    }
                    (_, Some(c)) => (format!("Last: eyes at {:.2} m, hands {:.2} m apart, {}.", c.head_height_m, c.arm_span_m, ago(c.utc_ms)), theme::FG_2),
                    (_, None) => ("Not calibrated yet.".into(), theme::FG_2),
                };
                let galley = p.layout_job(Txt::new(line, 15.0).color(tone).line_height(21.0).job(body.width()));
                p.galley(body.min, galley, tone);
                let hint = "Press Calibrate, then stand in a T-pose. Calibrate in VRChat at the same moment so both match.";
                let galley = p.layout_job(Txt::new(hint, 13.0).color(theme::FG_3).line_height(19.0).job(body.width() - 170.0));
                p.galley(pos2(body.left(), body.bottom() - 40.0), galley, theme::FG_3);
                let b = Button::new("Calibrate").icon(icons::PERSON).variant(Variant::Primary).enabled(snap.steamvr.running);
                let w = b.desired_width(ui).max(150.0);
                if b.width(w).show_at(ui, Rect::from_min_size(pos2(body.right() - w, body.bottom() - 40.0), vec2(w, 40.0))).clicked() {
                    self.start_calibration(now);
                }
            }
            Calibrating::Countdown { until } => {
                let left = (until - now).ceil().max(1.0) as u32;
                let k = 1.0 - ((until - now).fract() as f32);
                let big = Txt::new(left.to_string(), 64.0 + 8.0 * k).weight(weight::SEMIBOLD).color(theme::FG);
                widgets::text(&p, pos2(body.left() + 40.0, body.center().y), Align2::CENTER_CENTER, &big);
                widgets::text(
                    &p,
                    pos2(body.left() + 96.0, body.center().y - 12.0),
                    Align2::LEFT_CENTER,
                    &Txt::new("Get into a T-pose", 20.0).weight(weight::SEMIBOLD),
                );
                widgets::text(
                    &p,
                    pos2(body.left() + 96.0, body.center().y + 16.0),
                    Align2::LEFT_CENTER,
                    &Txt::new("Arms straight out, hands at shoulder height.", 14.0).color(theme::FG_2),
                );
            }
            Calibrating::Holding { check, .. } => {
                widgets::text(&p, pos2(body.left(), body.top() + 12.0), Align2::LEFT_CENTER, &Txt::new("Hold the pose…", 20.0).weight(weight::SEMIBOLD));
                let items = [
                    ("Arms out", check.wide),
                    ("Hands level", check.level),
                    ("At shoulders", check.shoulders),
                    ("Centered", check.centered),
                    ("Still", check.still),
                ];
                let w = body.width() / items.len() as f32;
                for (i, (label, ok)) in items.iter().enumerate() {
                    let c = pos2(body.left() + w * (i as f32 + 0.5), body.top() + 70.0);
                    let k = anim::toggle(ui.ctx(), Id::unique(("overlay-check", i)), *ok, Spring::BOUNCY);
                    let color = lerp(theme::FG_4, theme::OK, k.clamp(0.0, 1.0));
                    p.circle_filled(c, 15.0 + 2.0 * k, theme::alpha(color, 0.18));
                    widgets::icon(&p, c, if *ok { icons::CHECK } else { icons::CIRCLE }, 16.0, color);
                    widgets::text(&p, pos2(c.x, c.y + 30.0), Align2::CENTER_CENTER, &Txt::new(*label, 12.5).color(if *ok { theme::FG } else { theme::FG_3 }));
                }
            }
        }
    }
}

fn ago(utc_ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(utc_ms);
    let s = now.saturating_sub(utc_ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The desktop app's backdrop without its glow shader: ink with a violet corner. It never changes, so the painter
/// keeps it ([`orbiters_ui::software::Software::keep_as_backdrop`]) instead of drawing it every frame.
pub fn backdrop(ui: &Ui) {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme::INK_950);
    let mut mesh = egui::Mesh::default();
    let corner = |c: Color32, a: f32| theme::alpha(c, a);
    mesh.colored_vertex(rect.left_top(), corner(theme::VIOLET, 0.16));
    mesh.colored_vertex(rect.right_top(), corner(theme::INDIGO, 0.06));
    mesh.colored_vertex(rect.right_bottom(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.left_bottom(), corner(theme::BLUE, 0.05));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
}

fn header(ui: &mut Ui, snap: &Snapshot, rect: Rect) {
    let p = ui.painter().clone();
    let recording = matches!(snap.capture.state, CaptureState::Recording | CaptureState::Paused);
    let mark = Rect::from_min_size(pos2(28.0, 26.0), logo::size_for_width(44.0));
    logo::paint(&p, mark, logo::Look { pulse: recording, ..Default::default() });
    widgets::text(&p, pos2(mark.right() + 14.0, 38.0), Align2::LEFT_CENTER, &Txt::new("Orbiters", 22.0).weight(weight::SEMIBOLD).tracking(-0.01));
    let world = snap.vrchat.world.world_name.clone().filter(|_| snap.vrchat.world.in_world);
    let sub = match (&world, snap.vrchat.running) {
        (Some(w), _) => w.clone(),
        (None, true) => "VRChat is running".into(),
        (None, false) => "VRChat is not running".into(),
    };
    widgets::text_truncated(&p, pos2(mark.right() + 14.0, 62.0), Align2::LEFT_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), 420.0);

    // State on the right: REC with the elapsed time, paused, or ready.
    let (label, tone) = match snap.capture.state {
        CaptureState::Recording => ("REC", Tone::Rec),
        CaptureState::Paused => ("PAUSED", Tone::Warn),
        CaptureState::Starting => ("STARTING", Tone::Warn),
        CaptureState::Stopping => ("SAVING", Tone::Warn),
        CaptureState::Importing => ("IMPORTING", Tone::Warn),
        CaptureState::Idle => ("READY", Tone::Ok),
    };
    let elapsed = snap.capture.stats.as_ref().filter(|_| recording).map(|s| format::duration(s.elapsed_s));
    let lt = Txt::new(label, 13.0).weight(weight::SEMIBOLD).color(tone.color());
    let et = elapsed.as_ref().map(|e| Txt::new(e, 13.0).weight(weight::MEDIUM).color(theme::FG));
    let w = 14.0 + 10.0 + 10.0 + widgets::text_size(ui, &lt).x + et.as_ref().map(|t| 10.0 + widgets::text_size(ui, t).x).unwrap_or(0.0) + 16.0;
    let r = Rect::from_min_size(pos2(rect.right() - 24.0 - w, 30.0), vec2(w, 32.0));
    p.rect_filled(r, theme::radius(16.0), theme::alpha(tone.color(), 0.14));
    widgets::status_dot(&p, pos2(r.left() + 19.0, r.center().y), tone, snap.capture.state == CaptureState::Recording);
    let tr = widgets::text(&p, pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, &lt);
    if let Some(et) = et {
        widgets::text(&p, pos2(tr.right() + 10.0, r.center().y), Align2::LEFT_CENTER, &et);
    }
}

fn rec_state(s: CaptureState) -> RecState {
    match s {
        CaptureState::Idle => RecState::Idle,
        CaptureState::Starting => RecState::Starting,
        CaptureState::Recording => RecState::Recording,
        CaptureState::Paused => RecState::Paused,
        CaptureState::Stopping => RecState::Stopping,
        CaptureState::Importing => RecState::Importing,
    }
}

fn recorder(ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, card: Rect) {
    let c = &snap.capture;
    widgets::card(ui.painter(), card, theme::CARD_RADIUS);
    widgets::text(ui.painter(), pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("Recorder"));
    let consent = snap.settings.consent_acknowledged;
    let button = Rect::from_center_size(pos2(card.center().x, card.top() + 56.0 + record::SIZE * 0.5), Vec2::splat(record::SIZE));
    let health = c.stats.as_ref().map(|s| if s.frames > 0 { s.strip_ok as f32 / s.frames as f32 } else { 1.0 }).unwrap_or(1.0);
    let mut child = ui.new_child(UiBuilder::new().max_rect(card).id_salt("overlay-recorder"));
    let (_, toggled) = RecordButton { state: rec_state(c.state), disabled: !consent || c.state == CaptureState::Importing, health }.show_at(&mut child, button);
    if toggled {
        if snap.is_capturing() {
            ctl.stop_capture();
        } else {
            ctl.start_capture();
        }
    }
    let title = match c.state {
        CaptureState::Idle if !consent => "Accept the capture notice on the desktop first",
        CaptureState::Idle => "Start recording",
        CaptureState::Starting => "Starting…",
        CaptureState::Recording | CaptureState::Paused => "Stop recording",
        CaptureState::Stopping => "Saving the session…",
        CaptureState::Importing => "Importing…",
    };
    let p = ui.painter().clone();
    let y = button.bottom() + 16.0;
    widgets::text_truncated(&p, pos2(card.center().x, y + 8.0), Align2::CENTER_CENTER, &Txt::new(title, 18.0).weight(weight::SEMIBOLD), card.width() - 40.0);
    let sub = match (&c.source, c.state) {
        (Some(source), CaptureState::Recording | CaptureState::Paused) => source.clone(),
        _ if snap.osc.listening => "or use the Record toggle in your Action Menu".into(),
        _ => "Saved as a session for Mixer".into(),
    };
    widgets::text_truncated(&p, pos2(card.center().x, y + 34.0), Align2::CENTER_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), card.width() - 40.0);
    if matches!(c.state, CaptureState::Recording | CaptureState::Paused) {
        let paused = c.state == CaptureState::Paused;
        let b = Button::new(if paused { "Resume" } else { "Pause" }).icon(if paused { icons::PLAY } else { icons::PAUSE }).size(Size::Md);
        let w = b.desired_width(ui).max(140.0);
        if b.width(w).show_at(ui, Rect::from_min_size(pos2(card.center().x - w / 2.0, card.bottom() - 64.0), vec2(w, 40.0))).clicked() {
            ctl.pause_capture(!paused);
        }
    }
}

fn streams(ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, card: Rect) {
    widgets::card(ui.painter(), card, theme::CARD_RADIUS);
    widgets::text(ui.painter(), pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("What is recorded"));
    let capturing = snap.is_capturing();
    let row_h = 62.0;
    let mut changed: Option<Settings> = None;
    for (i, row) in crate::streams::streams(snap).iter().enumerate() {
        let r = Rect::from_min_size(pos2(card.left() + 12.0, card.top() + 50.0 + i as f32 * (row_h + 2.0)), vec2(card.width() - 24.0, row_h));
        let p = ui.painter().clone();
        if i > 0 {
            p.hline(r.x_range().shrink(10.0), r.top() - 1.0, egui::Stroke::new(1.0, theme::white(0.05)));
        }
        let tile = Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(40.0, 40.0));
        p.rect_filled(tile, theme::radius(12.0), theme::alpha(row.tone.color(), 0.12));
        widgets::icon(&p, tile.center(), row.glyph, 19.0, if row.tone == Tone::Off { theme::FG_3 } else { row.tone.color() });
        widgets::text(&p, pos2(tile.right() + 14.0, r.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(row.name, 15.5).weight(weight::SEMIBOLD));
        widgets::text_truncated(
            &p,
            pos2(tile.right() + 14.0, r.center().y + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&row.status, 13.0).color(theme::FG_3),
            r.width() - 190.0,
        );
        if let Some(level) = row.level {
            crate::streams::meter(
                ui,
                Id::unique(("overlay-meter", i)),
                Rect::from_min_size(pos2(r.right() - 150.0, r.center().y - 3.0), vec2(70.0, 6.0)),
                level,
            );
        }
        if let Some(on) = row.switch {
            let mut value = on;
            let mut sw = ui.new_child(
                UiBuilder::new().max_rect(Rect::from_min_size(pos2(r.right() - 58.0, r.center().y - 14.0), vec2(48.0, 28.0))).id_salt(("overlay-switch", i)),
            );
            if capturing {
                sw.disable();
            }
            if widgets::toggle(&mut sw, ("overlay-switch", i), &mut value) {
                changed = Some(crate::streams::with_switch(&snap.settings, i, value));
            }
        }
    }
    if let Some(s) = changed {
        ctl.save_settings(s);
    }
    if capturing {
        let p = ui.painter();
        widgets::text(
            p,
            pos2(card.right() - 22.0, card.top() + 28.0),
            Align2::RIGHT_CENTER,
            &Txt::new("Switches apply to the next recording", 12.0).color(theme::FG_4),
        );
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}
