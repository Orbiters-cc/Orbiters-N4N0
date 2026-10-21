//! What the SteamVR dashboard tab shows: the recorder, what each stream records right now, and the T-pose
//! calibration; or Orb: hold to talk, what Orb said, hear yourself. Laid out for a fixed 1024 × 640 point canvas, read
//! from a metre or two away.

use super::calibration::{Calibrator, Phase};
use crate::backend::Ctl;
use crate::ipc::OrbCommand;
use crate::state::{CaptureState, OrbWho, Snapshot};
use egui::{pos2, vec2, Align2, Id, Rect, Ui, UiBuilder, Vec2};
use orbiters_contracts::performance::tracking::{Device, Sample};
use orbiters_core::settings::Settings;
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Tone, Variant};
use orbiters_ui::{format, icons, logo};

pub const SIZE: Vec2 = Vec2::new(1024.0, 640.0);

/// Device poses for calibrating (SteamVR in the overlay, a stand-in in screenshots).
pub trait Poses {
    /// Connected devices, numbered by position, and a sample of each (play space, Unity conventions).
    fn read(&mut self) -> (Vec<Device>, Vec<Sample>);
}

/// The tab's two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Capture,
    Orb,
}

#[derive(Default)]
pub struct OverlayUi {
    pub calibrator: Calibrator,
    pub page: Page,
    /// The Talk button is held.
    talking: bool,
    /// "Hear yourself": the microphone played in the headset.
    hear: Option<orbiters_sound::MicMonitor>,
    hear_error: Option<String>,
}

impl OverlayUi {
    /// Whether frames must keep coming (a calibration under way).
    pub fn busy(&self) -> bool {
        self.calibrator.active()
    }

    /// Starts a calibration as if the button was pressed.
    pub fn start_calibration(&mut self, now: f64, poses: &mut dyn Poses) {
        let (devices, samples) = poses.read();
        self.calibrator.start(now, &devices, &samples);
    }

    /// Advances the calibration (every loop, the tab drawn or not) and saves it when a T-pose was held.
    pub fn tick(&mut self, now: f64, dashboard_visible: bool, poses: &mut dyn Poses, ctl: &Ctl) {
        if !self.calibrator.active() && !self.calibrator.in_world(now) && matches!(self.calibrator.phase, Phase::Idle) {
            return;
        }
        let (devices, samples) = poses.read();
        if let Some(c) = self.calibrator.tick(now, dashboard_visible, &devices, &samples) {
            ctl.calibrated(c);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, poses: &mut dyn Poses) {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
        header(ui, snap, rect);
        // Capture or Orb, in the middle of the header.
        let mut page = self.page;
        // The control's width: its labels with their padding, and the track's.
        let label = |t: &str| widgets::text_size(ui, &Txt::new(t, 13.0).weight(weight::MEDIUM)).x + 28.0;
        let track = label("Capture") + label("Orb") + 8.0;
        let mut tabs = ui.new_child(UiBuilder::new().max_rect(Rect::from_center_size(pos2(rect.center().x, 46.0), vec2(track, 36.0))).id_salt("overlay-pages"));
        widgets::segmented(&mut tabs, "overlay-page", &mut page, &[SegOption::new(Page::Capture, "Capture"), SegOption::new(Page::Orb, "Orb")], false);
        if page != self.page {
            self.page = page;
            if self.talking {
                self.talking = false;
                ctl.orb(OrbCommand::Talk { down: false });
            }
        }
        let left = Rect::from_min_size(pos2(24.0, 96.0), vec2(452.0, 520.0));
        let right = Rect::from_min_max(pos2(left.right() + 20.0, left.top()), pos2(rect.right() - 24.0, left.bottom()));
        if self.page == Page::Orb {
            self.orb(ui, snap, ctl, left, right);
            return;
        }
        recorder(ui, snap, ctl, left);
        let streams_rect = Rect::from_min_size(right.min, vec2(right.width(), 318.0));
        streams(ui, snap, ctl, streams_rect);
        let calib_rect = Rect::from_min_max(pos2(right.left(), streams_rect.bottom() + 16.0), right.max);
        self.calibration(ui, snap, poses, calib_rect);
    }

    fn calibration(&mut self, ui: &mut Ui, snap: &Snapshot, poses: &mut dyn Poses, rect: Rect) {
        let now = ui.input(|i| i.time);
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(rect.left() + 22.0, rect.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("T-pose calibration"));
        if self.busy() {
            ui.ctx().request_repaint();
        }
        let body = Rect::from_min_max(pos2(rect.left() + 22.0, rect.top() + 48.0), pos2(rect.right() - 22.0, rect.bottom() - 18.0));
        let button_rect = |w: f32| Rect::from_min_size(pos2(body.right() - w, body.bottom() - 40.0), vec2(w, 40.0));
        match self.calibrator.phase.clone() {
            Phase::Idle | Phase::Done { .. } | Phase::Failed { .. } => {
                let (line, tone) = match (&self.calibrator.phase, &snap.steamvr.calibration) {
                    (Phase::Failed { reason, .. }, _) => (reason.to_string(), theme::DANGER),
                    (Phase::Done { eye, span, .. }, _) => (format!("Calibrated: eyes at {eye:.2} m, hands {span:.2} m apart."), theme::OK),
                    (_, Some(c)) => (format!("Last: eyes at {:.2} m, hands {:.2} m apart, {}.", c.head_height_m, c.arm_span_m, ago(c.utc_ms)), theme::FG_2),
                    (_, None) => ("Not calibrated yet.".into(), theme::FG_2),
                };
                let galley = p.layout_job(Txt::new(line, 15.0).color(tone).line_height(21.0).job(body.width()));
                p.galley(body.min, galley, tone);
                let hint = "Not needed in VRChat: it sends your head and hands. In other games, press Calibrate and match the mirror.";
                let galley = p.layout_job(Txt::new(hint, 13.0).color(theme::FG_3).line_height(19.0).job(body.width() - 170.0));
                p.galley(pos2(body.left(), body.bottom() - 58.0), galley, theme::FG_3);
                let b = Button::new("Calibrate").icon(icons::PERSON).variant(Variant::Primary).enabled(snap.steamvr.running);
                let w = b.desired_width(ui).max(150.0);
                if b.width(w).show_at(ui, button_rect(w)).clicked() {
                    self.start_calibration(now, poses);
                }
            }
            phase => {
                let (big, title, line) = match phase {
                    Phase::Waiting { .. } => (None, "Close the dashboard".to_string(), "The mirror and the two hand targets are in front of you.".to_string()),
                    Phase::Countdown { until } => {
                        (Some((until - now).ceil().max(1.0) as u32), "Get into a T-pose".into(), "Hands on the two targets, arms straight.".into())
                    }
                    _ => (None, "Hold the pose…".into(), "Follow the mirror in front of you.".into()),
                };
                let x = if let Some(n) = big {
                    widgets::text(
                        &p,
                        pos2(body.left() + 30.0, body.top() + 50.0),
                        Align2::CENTER_CENTER,
                        &Txt::new(n.to_string(), 64.0).weight(weight::SEMIBOLD).color(theme::FG),
                    );
                    body.left() + 80.0
                } else {
                    body.left()
                };
                widgets::text(&p, pos2(x, body.top() + 38.0), Align2::LEFT_CENTER, &Txt::new(title, 20.0).weight(weight::SEMIBOLD));
                widgets::text(&p, pos2(x, body.top() + 66.0), Align2::LEFT_CENTER, &Txt::new(line, 14.0).color(theme::FG_2));
                let b = Button::new("Cancel").icon(icons::X).variant(Variant::Ghost);
                let w = b.desired_width(ui).max(130.0);
                if b.width(w).show_at(ui, button_rect(w)).clicked() {
                    self.calibrator.cancel();
                }
            }
        }
    }
}

impl OverlayUi {
    /// Orb's page: hold to talk on the left, what Orb said and its panel at the top right, quick actions under it.
    fn orb(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, left: Rect, right: Rect) {
        let orb = &snap.orb;
        let settings = &snap.settings.orb;
        let signed_in = matches!(
            snap.account.state,
            crate::state::AccountState::SignedIn { .. } | crate::state::AccountState::Offline { .. } | crate::state::AccountState::Checking { .. }
        );
        let can_talk = settings.enabled && settings.assistant && signed_in && orb.voice.installed;

        // Hold to talk: a round button that grows a ring with the voice.
        widgets::card(ui.painter(), left, theme::CARD_RADIUS);
        widgets::text(ui.painter(), pos2(left.left() + 22.0, left.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("Talk to Orb"));
        let center = pos2(left.center().x, left.top() + 56.0 + 100.0);
        let disc = Rect::from_center_size(center, Vec2::splat(200.0));
        let resp = ui.interact(disc, Id::unique("overlay-orb-talk"), egui::Sense::click_and_drag());
        let held = can_talk && resp.is_pointer_button_down_on();
        if held != self.talking {
            self.talking = held;
            ctl.orb(OrbCommand::Talk { down: held });
        }
        let p = ui.painter().clone();
        let level = if orb.listening { orb.voice.level.clamp(0.0, 1.0) } else { 0.0 };
        if orb.listening || orb.thinking {
            p.circle_filled(center, 100.0 + 14.0 * level, theme::alpha(theme::ACCENT, 0.10 + 0.15 * level));
        }
        let fill = if !can_talk {
            theme::INK_750
        } else if held || orb.listening {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        };
        p.circle_filled(center, if held { 84.0 } else { 88.0 }, fill);
        widgets::icon(&p, center, icons::MICROPHONE, 64.0, if can_talk { theme::ON_ACCENT } else { theme::FG_3 });
        let (title, sub) = if !settings.enabled || !settings.assistant {
            ("Orb's assistant is off", "Turn it on in Orbiters desktop › Orb".to_string())
        } else if !signed_in {
            ("Sign in to talk to Orb", "In Orbiters desktop › Settings".to_string())
        } else if !orb.voice.installed {
            ("Orb cannot hear yet", "Install its ears in Orbiters desktop › Orb".to_string())
        } else if orb.listening {
            ("Listening…", "Let go when you are done".to_string())
        } else if orb.thinking {
            ("Thinking…", "Orb answers on its panel".to_string())
        } else {
            ("Hold and speak", "Or hold Talk in Orb's menu, or touch the orb".to_string())
        };
        let y = disc.bottom() + 28.0;
        widgets::text_truncated(&p, pos2(left.center().x, y), Align2::CENTER_CENTER, &Txt::new(title, 20.0).weight(weight::SEMIBOLD), left.width() - 40.0);
        widgets::text_truncated(&p, pos2(left.center().x, y + 28.0), Align2::CENTER_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), left.width() - 40.0);
        if let Some(q) = orb.quota.as_ref().filter(|q| q.limit > 0 && !q.own_key) {
            let line = format!("{} of {} free answers left today", q.limit.saturating_sub(q.used), q.limit);
            widgets::text(&p, pos2(left.center().x, left.bottom() - 28.0), Align2::CENTER_CENTER, &Txt::new(line, 12.5).color(theme::FG_4));
        }
        if orb.listening || orb.thinking || held {
            ui.ctx().request_repaint();
        }

        // What Orb said, and its panel.
        let said = Rect::from_min_size(right.min, vec2(right.width(), 318.0));
        widgets::card(ui.painter(), said, theme::CARD_RADIUS);
        widgets::text(ui.painter(), pos2(said.left() + 22.0, said.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps("Orb said"));
        let body = Rect::from_min_max(pos2(said.left() + 22.0, said.top() + 48.0), pos2(said.right() - 22.0, said.bottom() - 18.0));
        let mut yy = body.top();
        let last: Vec<_> = orb.lines.iter().filter(|l| matches!(l.who, OrbWho::You | OrbWho::Orb | OrbWho::Error)).rev().take(3).collect();
        if last.is_empty() {
            let g = p.layout_job(
                Txt::new("Nothing yet. Ask Orb to put your hoodie on, take a picture, or plan a movie night.", 15.0)
                    .color(theme::FG_3)
                    .line_height(21.0)
                    .job(body.width()),
            );
            p.galley(body.min, g, theme::FG_3);
        }
        for line in last.into_iter().rev() {
            let (who, color) = match line.who {
                OrbWho::You => ("You", theme::FG_3),
                OrbWho::Error => ("Orb", theme::DANGER),
                _ => ("Orb", theme::ACCENT),
            };
            widgets::text(&p, pos2(body.left(), yy + 9.0), Align2::LEFT_CENTER, &Txt::caps(who).color(color));
            let g = p.layout_job(
                Txt::new(&line.text, 15.0).color(if line.who == OrbWho::You { theme::FG_2 } else { theme::FG }).line_height(21.0).job(body.width() - 56.0),
            );
            let h = g.size().y.min(64.0);
            if yy + h > body.bottom() {
                break;
            }
            p.galley(pos2(body.left() + 56.0, yy), g, theme::FG);
            yy += h + 14.0;
        }

        // Quick actions.
        let quick = Rect::from_min_max(pos2(right.left(), said.bottom() + 16.0), right.max);
        widgets::card(ui.painter(), quick, theme::CARD_RADIUS);
        let rows = Rect::from_min_max(pos2(quick.left() + 22.0, quick.top() + 18.0), pos2(quick.right() - 22.0, quick.bottom() - 18.0));
        let row_h = 44.0;
        let row = |i: usize| Rect::from_min_size(pos2(rows.left(), rows.top() + i as f32 * (row_h + 8.0)), vec2(rows.width(), row_h));
        // Hear yourself: the microphone in the headset.
        let r0 = row(0);
        widgets::text(&p, pos2(r0.left(), r0.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new("Hear yourself", 15.5).weight(weight::SEMIBOLD));
        let hint = self.hear_error.clone().unwrap_or_else(|| "Your microphone in the headset, as others hear it".into());
        widgets::text_truncated(&p, pos2(r0.left(), r0.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(hint, 12.5).color(theme::FG_3), r0.width() - 80.0);
        let mut on = self.hear.is_some();
        let mut sw = ui
            .new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(r0.right() - 48.0, r0.center().y - 14.0), vec2(48.0, 28.0))).id_salt("overlay-hear"));
        if widgets::toggle(&mut sw, "overlay-hear", &mut on) {
            self.hear = None;
            self.hear_error = None;
            if on {
                let device = (!settings.mic_device.is_empty()).then(|| settings.mic_device.clone());
                match orbiters_sound::MicMonitor::start(device) {
                    Ok(m) => self.hear = Some(m),
                    Err(e) => self.hear_error = Some(format!("The microphone did not start: {e:#}")),
                }
            }
        }
        // AFK message.
        let r1 = row(1);
        widgets::text(&p, pos2(r1.left(), r1.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new("AFK message", 15.5).weight(weight::SEMIBOLD));
        widgets::text_truncated(
            &p,
            pos2(r1.left(), r1.center().y + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&settings.afk_message, 12.5).color(theme::FG_3),
            r1.width() - 80.0,
        );
        let mut afk = settings.afk_message_enabled;
        let mut sw = ui
            .new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(r1.right() - 48.0, r1.center().y - 14.0), vec2(48.0, 28.0))).id_salt("overlay-afk"));
        if widgets::toggle(&mut sw, "overlay-afk", &mut afk) {
            let mut s = snap.settings.clone();
            s.orb.afk_message_enabled = afk;
            ctl.save_settings(s);
        }
        // Buttons.
        let r2 = row(2);
        let save = Button::new("Save this video").icon(icons::DOWNLOAD_SIMPLE).enabled(snap.vrchat.world.last_video.is_some());
        let w = save.desired_width(ui).max(180.0);
        if save.width(w).show_at(ui, Rect::from_min_size(pos2(r2.left(), r2.top() + 2.0), vec2(w, 40.0))).clicked() {
            ctl.orb(OrbCommand::SaveVideo);
        }
        let clear = Button::new("Clear the panel").icon(icons::X).variant(Variant::Ghost).enabled(orb.panel.is_some());
        let w2 = clear.desired_width(ui).max(170.0);
        if clear.width(w2).show_at(ui, Rect::from_min_size(pos2(r2.left() + w + 12.0, r2.top() + 2.0), vec2(w2, 40.0))).clicked() {
            ctl.orb(OrbCommand::HidePanel);
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

/// The tab's backdrop: flat ink, as the website's page. It never changes, so the painter
/// keeps it ([`orbiters_ui::software::Software::keep_as_backdrop`]) instead of drawing it every frame.
pub fn backdrop(ui: &Ui) {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme::INK_950);
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
