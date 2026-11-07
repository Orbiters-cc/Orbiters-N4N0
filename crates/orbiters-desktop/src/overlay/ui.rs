//! What the SteamVR dashboard tab shows: the recorder, what each stream records right now, and the T-pose
//! calibration; or N4N0: hold to talk, what N4N0 said, hear yourself. Laid out for a fixed 1024 × 640 point canvas, read
//! from a metre or two away.

use super::calibration::{metres, Calibrator, Phase};
use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::state::{CaptureState, N4n0Who, Snapshot};
use egui::{pos2, vec2, Align2, Id, Rect, Ui, UiBuilder, Vec2};
use orbiters_contracts::performance::tracking::{Device, Sample};
use orbiters_core::settings::Settings;
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Tone, Variant};
use orbiters_ui::{format, icons, n4n0_logo};

pub const SIZE: Vec2 = Vec2::new(1024.0, 640.0);

/// Device poses for calibrating (SteamVR in the overlay, a stand-in in screenshots).
pub trait Poses {
    /// Connected devices, numbered by position, and a sample of each (play space, Unity conventions).
    fn read(&mut self) -> (Vec<Device>, Vec<Sample>);
}

/// The tab's pages: the recorder, N4N0, walk boost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Capture,
    N4n0,
    Walk,
}

/// Where the interface shows: the tab in SteamVR's dashboard, or the floating panel in the play space
/// ([`super::floating`]), beside which VRChat keeps its controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Dashboard,
    Floating,
}

#[derive(Default)]
pub struct OverlayUi {
    pub calibrator: Calibrator,
    pub page: Page,
    /// The tab's Float button was pressed: the floating panel opens before the eyes.
    float: bool,
    /// The Talk button is held.
    talking: bool,
    /// "Hear yourself": the microphone played in the headset.
    hear: Option<orbiters_sound::MicMonitor>,
    /// Its devices opening, on a thread of their own (up to 3 s that the overlay's loop does not wait).
    hear_starting: Option<crate::pages::Job<Result<orbiters_sound::MicMonitor, String>>>,
    hear_error: Option<String>,
    /// Why walk boost cannot work here (SteamVR input refused the actions), if it cannot.
    pub walk_unavailable: Option<String>,
    /// Another application moved the play space lately, and the distance walked went with it.
    pub walk_taken: bool,
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

    /// The tab stops seeing the laser (the dashboard hid, the laser left it, the overlay is going): a held Talk
    /// button is let go, as the press's end will never come.
    pub fn release(&mut self, ctl: &Ctl) {
        if std::mem::take(&mut self.talking) {
            ctl.n4n0(N4n0Command::Talk { down: false });
        }
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

    /// The tab's Float button was pressed since the last call.
    pub fn take_float(&mut self) -> bool {
        std::mem::take(&mut self.float)
    }

    /// The interface in the dashboard's tab.
    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, poses: &mut dyn Poses) {
        self.show_in(ui, snap, ctl, poses, Surface::Dashboard);
    }

    /// The interface on `surface`: the same on both but for the tab's Float button, which opens the floating panel
    /// (VRChat keeps its controls while that shows; the dashboard takes them).
    pub fn show_in(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, poses: &mut dyn Poses, surface: Surface) {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
        let pill = header(ui, snap, rect);
        if surface == Surface::Dashboard {
            let float = Button::new(tr("Float")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost).size(Size::Sm).id_salt("overlay-float");
            let w = float.desired_width(ui);
            let at = Rect::from_min_size(pos2(pill.left() - 10.0 - w, pill.center().y - 16.0), vec2(w, 32.0));
            if float.hint(tr("In the play space beside VRChat, which keeps your controllers")).show_at(ui, at).clicked() {
                self.float = true;
            }
        }
        // Capture or N4N0, in the middle of the header.
        let mut page = self.page;
        // The control's width: its labels with their padding, and the track's.
        let label = |t: &str| widgets::text_size(ui, &Txt::new(t, 13.0).weight(weight::MEDIUM)).x + 28.0;
        let track = label(tr("Capture")) + label("N4N0") + label(tr("Walk boost")) + 8.0;
        let mut tabs = ui.new_child(UiBuilder::new().max_rect(Rect::from_center_size(pos2(rect.center().x, 46.0), vec2(track, 36.0))).id_salt("overlay-pages"));
        widgets::segmented(
            &mut tabs,
            "overlay-page",
            &mut page,
            &[SegOption::new(Page::Capture, tr("Capture")), SegOption::new(Page::N4n0, "N4N0"), SegOption::new(Page::Walk, tr("Walk boost"))],
            false,
        );
        if page != self.page {
            self.page = page;
            if self.talking {
                self.talking = false;
                ctl.n4n0(N4n0Command::Talk { down: false });
            }
        }
        let left = Rect::from_min_size(pos2(24.0, 96.0), vec2(452.0, 520.0));
        let right = Rect::from_min_max(pos2(left.right() + 20.0, left.top()), pos2(rect.right() - 24.0, left.bottom()));
        if self.page == Page::N4n0 {
            self.n4n0(ui, snap, ctl, left, right);
            return;
        }
        if self.page == Page::Walk {
            self.walk(ui, snap, ctl, left, right);
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
        widgets::text(&p, pos2(rect.left() + 22.0, rect.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("T-pose calibration")));
        if self.busy() {
            ui.ctx().request_repaint();
        }
        let body = Rect::from_min_max(pos2(rect.left() + 22.0, rect.top() + 48.0), pos2(rect.right() - 22.0, rect.bottom() - 18.0));
        let button_rect = |w: f32| Rect::from_min_size(pos2(body.right() - w, body.bottom() - 40.0), vec2(w, 40.0));
        match self.calibrator.phase.clone() {
            Phase::Idle | Phase::Done { .. } | Phase::Failed { .. } => {
                let (line, tone) = match (&self.calibrator.phase, &snap.steamvr.calibration) {
                    (Phase::Failed { reason, .. }, _) => (tr(reason).to_string(), theme::DANGER),
                    (Phase::Done { eye, span, .. }, _) => (tr!("Calibrated: eyes at {eye} m, hands {span} m apart.", eye = metres(*eye), span = metres(*span)), theme::OK),
                    (_, Some(c)) => (
                        tr!("Last: eyes at {eye} m, hands {span} m apart, {ago}.", eye = metres(c.head_height_m), span = metres(c.arm_span_m), ago = ago(c.utc_ms)),
                        theme::FG_2,
                    ),
                    (_, None) => (tr("Not calibrated yet.").into(), theme::FG_2),
                };
                let galley = p.layout_job(Txt::new(line, 15.0).color(tone).line_height(21.0).job(body.width()));
                p.galley(body.min, galley, tone);
                let hint = tr("Not needed in VRChat: it sends your head and hands. In other games, press Calibrate and match the mirror.");
                let galley = p.layout_job(Txt::new(hint, 13.0).color(theme::FG_3).line_height(19.0).job(body.width() - 170.0));
                p.galley(pos2(body.left(), body.bottom() - 58.0), galley, theme::FG_3);
                let b = Button::new(tr("Calibrate")).icon(icons::PERSON).variant(Variant::Primary).enabled(snap.steamvr.running);
                let w = b.desired_width(ui).max(150.0);
                if b.width(w).show_at(ui, button_rect(w)).clicked() {
                    self.start_calibration(now, poses);
                }
            }
            phase => {
                let (big, title, line) = match phase {
                    Phase::Waiting { .. } => (None, tr("Close the dashboard").to_string(), tr("The mirror and the two hand targets are in front of you.").to_string()),
                    Phase::Countdown { until } => {
                        (Some((until - now).ceil().max(1.0) as u32), tr("Get into a T-pose").into(), tr("Hands on the two targets, arms straight.").into())
                    }
                    _ => (None, tr("Hold the pose…").into(), tr("Follow the mirror in front of you.").into()),
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
                let b = Button::new(tr("Cancel")).icon(icons::X).variant(Variant::Ghost);
                let w = b.desired_width(ui).max(130.0);
                if b.width(w).show_at(ui, button_rect(w)).clicked() {
                    self.calibrator.cancel();
                }
            }
        }
    }
}

impl OverlayUi {
    /// N4N0's page: hold to talk on the left, what N4N0 said and its panel at the top right, quick actions under it.
    /// Walk boost: on or off and how strong (left), which stick and which way is forward (right).
    fn walk(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, left: Rect, right: Rect) {
        use orbiters_core::settings::{Follow, Hand, WalkBoostSettings};
        let boost = &snap.settings.walk_boost;
        let p = ui.painter().clone();
        widgets::card(&p, left, theme::CARD_RADIUS);
        widgets::text(&p, pos2(left.left() + 22.0, left.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("Walk boost")));
        let extra = (boost.strength * WalkBoostSettings::MAX_EXTRA * 100.0).round();
        let big = if boost.enabled && boost.strength > 0.0 { format!("+{extra:.0} %") } else { tr("Off").to_string() };
        let center = pos2(left.center().x, left.top() + 150.0);
        let tone = if boost.enabled && boost.strength > 0.0 { theme::ACCENT } else { theme::FG_3 };
        widgets::text(&p, center, Align2::CENTER_CENTER, &Txt::new(big, 64.0).weight(weight::SEMIBOLD).color(tone));
        let (line, color) = if let Some(why) = &self.walk_unavailable {
            (tr!("SteamVR refused the controls: {why}", why = why), theme::WARN)
        } else if self.walk_taken {
            (tr("Another app moved the play space, so you went back.").to_string(), theme::WARN)
        } else if let Some(words) = boost.enabled.then(|| snap.steamvr.walk_path.and_then(crate::state::WalkPath::lesser_short)).flatten() {
            (tr(words).to_string(), theme::WARN)
        } else if !snap.vrchat.running {
            (tr("Works in VRChat: start it to walk faster.").to_string(), theme::FG_3)
        } else if !snap.vrchat.world.in_world {
            (tr("Works once you are in a world.").to_string(), theme::FG_3)
        } else {
            (tr("Push your stick: the room moves with you.").to_string(), theme::FG_3)
        };
        widgets::text_truncated(&p, pos2(left.center().x, center.y + 62.0), Align2::CENTER_CENTER, &Txt::new(line, 14.0).color(color), left.width() - 40.0);

        // On or off, then the strength.
        let row = |y: f32| Rect::from_min_size(pos2(left.left() + 22.0, y), vec2(left.width() - 44.0, 40.0));
        let on_row = row(left.top() + 290.0);
        widgets::text(&p, on_row.left_center(), Align2::LEFT_CENTER, &Txt::new(tr("Boost my walking"), 16.0).weight(weight::MEDIUM));
        let mut on = boost.enabled;
        let mut child = ui.new_child(UiBuilder::new().max_rect(on_row).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if widgets::toggle(&mut child, "overlay-walk-on", &mut on) {
            let mut settings = snap.settings.clone();
            settings.walk_boost.enabled = on;
            ctl.save_changes(&snap.settings, &settings);
        }
        let strength_row = row(on_row.bottom() + 24.0);
        widgets::text(&p, strength_row.left_center(), Align2::LEFT_CENTER, &Txt::new(tr("Strength"), 16.0).weight(weight::MEDIUM));
        let mut strength = boost.strength;
        let mut child = ui.new_child(UiBuilder::new().max_rect(strength_row).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.add_enabled_ui(boost.enabled, |ui| {
            if widgets::slider(ui, "overlay-walk-strength", &mut strength, 0.0..=1.0, 0.05, |v| format!("+{:.0} %", v * WalkBoostSettings::MAX_EXTRA * 100.0)) {
                ctl.walk_boost(strength);
            }
        });
        widgets::text_truncated(
            &p,
            pos2(left.left() + 22.0, left.bottom() - 28.0),
            Align2::LEFT_CENTER,
            &Txt::new(tr("N4N0's Walk boost radial on your avatar sets it too."), 12.5).color(theme::FG_4),
            left.width() - 44.0,
        );

        // The stick and the way forward.
        widgets::card(&p, right, theme::CARD_RADIUS);
        widgets::text(&p, pos2(right.left() + 22.0, right.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("Your stick")));
        let mut y = right.top() + 64.0;
        let choice = |ui: &mut Ui, title: &str, sub: &str, y: f32, draw: &mut dyn FnMut(&mut Ui)| {
            widgets::text(&p, pos2(right.left() + 22.0, y + 12.0), Align2::LEFT_CENTER, &Txt::new(title, 16.0).weight(weight::MEDIUM));
            widgets::text_truncated(
                &p,
                pos2(right.left() + 22.0, y + 36.0),
                Align2::LEFT_CENTER,
                &Txt::new(sub, 13.0).color(theme::FG_3),
                right.width() - 44.0,
            );
            let area = Rect::from_min_size(pos2(right.left() + 22.0, y + 56.0), vec2(right.width() - 44.0, 40.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)));
            draw(&mut child);
        };
        let mut hand = boost.hand;
        choice(ui, tr("Walking hand"), tr("The hand whose stick moves you in VRChat."), y, &mut |ui| {
            if widgets::segmented(ui, "overlay-walk-hand", &mut hand, &[SegOption::new(Hand::Left, tr("Left")), SegOption::new(Hand::Right, tr("Right"))], true) {
                let mut settings = snap.settings.clone();
                settings.walk_boost.hand = hand;
                ctl.save_changes(&snap.settings, &settings);
            }
        });
        y += 130.0;
        let mut follow = boost.follow;
        choice(ui, tr("Forward is"), tr("Where you look (VRChat's default), or where that hand points."), y, &mut |ui| {
            if widgets::segmented(
                ui,
                "overlay-walk-follow",
                &mut follow,
                &[SegOption::new(Follow::Head, tr("Where I look")), SegOption::new(Follow::Hand, tr("Where my hand points"))],
                true,
            ) {
                let mut settings = snap.settings.clone();
                settings.walk_boost.follow = follow;
                ctl.save_changes(&snap.settings, &settings);
            }
        });
        y += 130.0;
        let g = p.layout_job(
            Txt::new(tr("Your play space follows you, and the chaperone stays on your room's walls. It goes back when you change world."), 13.5)
                .color(theme::FG_3)
                .line_height(19.0)
                .job(right.width() - 44.0),
        );
        p.galley(pos2(right.left() + 22.0, y + 8.0), g, theme::FG_3);
    }

    fn n4n0(&mut self, ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, left: Rect, right: Rect) {
        let n4n0 = &snap.n4n0;
        let settings = &snap.settings.n4n0;
        let signed_in = matches!(
            snap.account.state,
            crate::state::AccountState::SignedIn { .. } | crate::state::AccountState::Offline { .. } | crate::state::AccountState::Checking { .. }
        );
        let can_talk = settings.enabled && settings.assistant && signed_in && n4n0.voice.installed;

        // Hold to talk: a round button that grows a ring with the voice.
        widgets::card(ui.painter(), left, theme::CARD_RADIUS);
        widgets::text(ui.painter(), pos2(left.left() + 22.0, left.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("Talk to N4N0")));
        let center = pos2(left.center().x, left.top() + 56.0 + 100.0);
        let disc = Rect::from_center_size(center, Vec2::splat(200.0));
        let resp = ui.interact(disc, Id::unique("overlay-n4n0-talk"), egui::Sense::click_and_drag());
        let held = can_talk && resp.is_pointer_button_down_on();
        if held != self.talking {
            self.talking = held;
            ctl.n4n0(N4n0Command::Talk { down: held });
        }
        let p = ui.painter().clone();
        let level = if n4n0.listening { n4n0.voice.level.clamp(0.0, 1.0) } else { 0.0 };
        if n4n0.listening || n4n0.thinking {
            p.circle_filled(center, 100.0 + 14.0 * level, theme::alpha(theme::ACCENT, 0.10 + 0.15 * level));
        }
        let fill = if !can_talk {
            theme::INK_750
        } else if held || n4n0.listening {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        };
        p.circle_filled(center, if held { 84.0 } else { 88.0 }, fill);
        widgets::icon(&p, center, icons::MICROPHONE, 64.0, if can_talk { theme::ON_ACCENT } else { theme::FG_3 });
        let (title, sub) = if !settings.enabled || !settings.assistant {
            (tr("N4N0's assistant is off"), tr("Turn it on in Orbiters - N4N0 › N4N0"))
        } else if !signed_in {
            (tr("Sign in to talk to N4N0"), tr("In Orbiters - N4N0 › Settings"))
        } else if !n4n0.voice.installed {
            (tr("N4N0 cannot hear yet"), tr("Install its ears in Orbiters - N4N0 › N4N0"))
        } else if n4n0.listening {
            (tr("Listening…"), tr("Let go when you are done"))
        } else if n4n0.thinking {
            (tr("Thinking…"), tr("N4N0 answers on its panel"))
        } else {
            (tr("Hold and speak"), tr("Or hold Talk in N4N0's menu, or touch N4N0"))
        };
        let y = disc.bottom() + 28.0;
        widgets::text_truncated(&p, pos2(left.center().x, y), Align2::CENTER_CENTER, &Txt::new(title, 20.0).weight(weight::SEMIBOLD), left.width() - 40.0);
        widgets::text_truncated(&p, pos2(left.center().x, y + 28.0), Align2::CENTER_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), left.width() - 40.0);
        let quota = n4n0.quota.as_ref().filter(|q| q.limit > 0 && !q.own_key);
        if let Some(q) = quota {
            let line = tr!("{left} of {limit} free answers left today", left = q.limit.saturating_sub(q.used), limit = q.limit);
            widgets::text(&p, pos2(left.center().x, left.bottom() - 28.0), Align2::CENTER_CENTER, &Txt::new(line, 12.5).color(theme::FG_4));
        }
        // VRChat sends avatar parameters only to OSC services it found: without them, a hand on N4N0 and the menu's
        // Talk button never arrive.
        let quiet = snap.vrchat.running && snap.osc.enabled && !snap.osc.receiving;
        if quiet {
            let g = p.layout_job(
                Txt::new(tr("VRChat sends Orbiters nothing: touching N4N0 and Talk in its menu cannot reach N4N0. Is OSC on in VRChat's Action Menu?"), 12.5)
                    .color(theme::WARN)
                    .line_height(17.0)
                    .job(left.width() - 44.0),
            );
            p.galley(pos2(left.left() + 22.0, y + 52.0), g, theme::WARN);
        }
        // N4N0's menu from here too (it opens in front of you), and asking VRChat again when it is quiet.
        let by = left.bottom() - if quota.is_some() { 96.0 } else { 64.0 };
        let menu_button = Button::new(tr("N4N0's menu")).icon(icons::STACK).variant(Variant::Secondary).enabled(settings.enabled);
        let w = menu_button.desired_width(ui).max(150.0);
        let reconnect_w = 150.0;
        let x0 = left.center().x - (w + if quiet { 12.0 + reconnect_w } else { 0.0 }) / 2.0;
        if menu_button.width(w).show_at(ui, Rect::from_min_size(pos2(x0, by), vec2(w, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::ToggleScreen);
        }
        if quiet {
            let b = Button::new(tr("Reconnect")).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Ghost);
            if b.width(reconnect_w).show_at(ui, Rect::from_min_size(pos2(x0 + w + 12.0, by), vec2(reconnect_w, 40.0))).clicked() {
                ctl.osc_reconnect();
            }
        }
        if n4n0.listening || n4n0.thinking || held {
            ui.ctx().request_repaint();
        }

        // What N4N0 said, and its panel.
        let said = Rect::from_min_size(right.min, vec2(right.width(), 318.0));
        widgets::card(ui.painter(), said, theme::CARD_RADIUS);
        widgets::text(ui.painter(), pos2(said.left() + 22.0, said.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("N4N0 said")));
        let body = Rect::from_min_max(pos2(said.left() + 22.0, said.top() + 48.0), pos2(said.right() - 22.0, said.bottom() - 18.0));
        let mut yy = body.top();
        let last: Vec<_> = n4n0.lines.iter().filter(|l| matches!(l.who, N4n0Who::You | N4n0Who::N4n0 | N4n0Who::Error)).rev().take(3).collect();
        if last.is_empty() {
            let g = p.layout_job(
                Txt::new(tr("Nothing yet. Ask N4N0 to put your hoodie on, take a picture, or plan a movie night."), 15.0)
                    .color(theme::FG_3)
                    .line_height(21.0)
                    .job(body.width()),
            );
            p.galley(body.min, g, theme::FG_3);
        }
        for line in last.into_iter().rev() {
            let (who, color) = match line.who {
                N4n0Who::You => (tr("You"), theme::FG_3),
                N4n0Who::Error => ("N4N0", theme::DANGER),
                _ => ("N4N0", theme::ACCENT),
            };
            widgets::text(&p, pos2(body.left(), yy + 9.0), Align2::LEFT_CENTER, &Txt::caps(who).color(color));
            let g = p.layout_job(
                Txt::new(&line.text, 15.0).color(if line.who == N4n0Who::You { theme::FG_2 } else { theme::FG }).line_height(21.0).job(body.width() - 56.0),
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
        widgets::text(&p, pos2(r0.left(), r0.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(tr("Hear yourself"), 15.5).weight(weight::SEMIBOLD));
        if let Some(result) = self.hear_starting.as_mut().and_then(|job| job.take()) {
            self.hear_starting = None;
            match result {
                Ok(m) => self.hear = Some(m),
                Err(e) => self.hear_error = Some(e),
            }
        }
        // The error is kept as it came and put in its sentence here, so the sentence follows the language.
        let hint = match &self.hear_error {
            Some(e) => tr!("The microphone did not start: {e}", e = e),
            None => tr("Your microphone in the headset, as others hear it").to_string(),
        };
        widgets::text_truncated(&p, pos2(r0.left(), r0.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(hint, 12.5).color(theme::FG_3), r0.width() - 80.0);
        let mut on = self.hear.is_some() || self.hear_starting.is_some();
        let mut sw = ui
            .new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(r0.right() - 48.0, r0.center().y - 14.0), vec2(48.0, 28.0))).id_salt("overlay-hear"));
        if widgets::toggle(&mut sw, "overlay-hear", &mut on) {
            self.hear = None;
            // A start still under way is let go: its monitor stops as soon as it opens.
            self.hear_starting = None;
            self.hear_error = None;
            if on {
                let device = (!settings.mic_device.is_empty()).then(|| settings.mic_device.clone());
                self.hear_starting =
                    Some(crate::pages::Job::spawn(ui.ctx(), move || orbiters_sound::MicMonitor::start(device).map_err(|e| format!("{e:#}"))));
            }
        }
        // AFK message.
        let r1 = row(1);
        widgets::text(&p, pos2(r1.left(), r1.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(tr("AFK message"), 15.5).weight(weight::SEMIBOLD));
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
            s.n4n0.afk_message_enabled = afk;
            ctl.save_changes(&snap.settings, &s);
        }
        // Buttons.
        let r2 = row(2);
        let call = snap.n4n0.calls.call.as_ref();
        if let Some(call) = call {
            let ringing = call.incoming && call.phase == crate::state::CallPhase::Ringing;
            let (first, first_cmd) = if ringing { (tr("Answer"), N4n0Command::AnswerCall) } else { (tr("Hang up"), N4n0Command::EndCall) };
            let b = Button::new(first).icon(if ringing { icons::MICROPHONE } else { icons::X }).variant(if ringing {
                Variant::Primary
            } else {
                Variant::Secondary
            });
            let w1 = b.desired_width(ui).max(150.0);
            if b.width(w1).show_at(ui, Rect::from_min_size(pos2(r2.left(), r2.top() + 2.0), vec2(w1, 40.0))).clicked() {
                ctl.n4n0(first_cmd);
            }
            if ringing {
                let d = Button::new(tr_ctx("a call", "Decline")).icon(icons::X).variant(Variant::Ghost);
                let w2 = d.desired_width(ui).max(120.0);
                if d.width(w2).show_at(ui, Rect::from_min_size(pos2(r2.left() + w1 + 12.0, r2.top() + 2.0), vec2(w2, 40.0))).clicked() {
                    ctl.n4n0(N4n0Command::EndCall);
                }
            }
            return;
        }
        let save = Button::new(tr("Save this video")).icon(icons::DOWNLOAD_SIMPLE).enabled(snap.vrchat.world.last_video.is_some());
        let w = save.desired_width(ui).max(180.0);
        if save.width(w).show_at(ui, Rect::from_min_size(pos2(r2.left(), r2.top() + 2.0), vec2(w, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::SaveVideo);
        }
        let photo = Button::new(tr("Picture in 3 s")).icon(icons::CAMERA).enabled(snap.vrchat.running && !snap.is_capturing());
        let w2 = photo.desired_width(ui).max(160.0);
        if photo.width(w2).show_at(ui, Rect::from_min_size(pos2(r2.left() + w + 12.0, r2.top() + 2.0), vec2(w2, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::Photo { delay_seconds: 3 });
        }
        let clear = Button::new("").icon(icons::X).variant(Variant::Ghost).hint(tr("Clear the panel")).enabled(n4n0.panel.is_some());
        if clear.width(48.0).show_at(ui, Rect::from_min_size(pos2(r2.right() - 48.0, r2.top() + 2.0), vec2(48.0, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::HidePanel);
        }
    }
}

/// How long ago `utc_ms` was, as the desktop window says it.
fn ago(utc_ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(utc_ms / 1000, |d| d.as_secs());
    format::relative_time(utc_ms / 1000, now)
}

/// The tab's backdrop: flat ink, as the website's page. It never changes, so the painter
/// keeps it ([`orbiters_ui::software::Software::keep_as_backdrop`]) instead of drawing it every frame.
pub fn backdrop(ui: &Ui) {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), SIZE);
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme::INK_950);
}

/// The header; returns where its state pill is (the Float button goes left of it).
fn header(ui: &mut Ui, snap: &Snapshot, rect: Rect) -> Rect {
    let p = ui.painter().clone();
    let recording = matches!(snap.capture.state, CaptureState::Recording | CaptureState::Paused);
    let mark = Rect::from_min_size(pos2(28.0, 26.0), n4n0_logo::size_for_width(44.0));
    n4n0_logo::paint(&p, mark, theme::MARK);
    widgets::text(&p, pos2(mark.right() + 14.0, 38.0), Align2::LEFT_CENTER, &Txt::new("N4N0", 22.0).weight(weight::SEMIBOLD).tracking(0.02));
    let world = snap.vrchat.world.world_name.clone().filter(|_| snap.vrchat.world.in_world);
    let sub = match (&world, snap.vrchat.running) {
        (Some(w), _) => w.clone(),
        (None, true) => tr("VRChat is running").into(),
        (None, false) => tr("VRChat is not running").into(),
    };
    widgets::text_truncated(&p, pos2(mark.right() + 14.0, 62.0), Align2::LEFT_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), 420.0);

    // State on the right: REC with the elapsed time, paused, or ready.
    let (label, tone) = match snap.capture.state {
        CaptureState::Recording => (tr("REC"), Tone::Rec),
        CaptureState::Paused => (tr("PAUSED"), Tone::Warn),
        CaptureState::Starting => (tr("STARTING"), Tone::Warn),
        CaptureState::Stopping => (tr("SAVING"), Tone::Warn),
        CaptureState::Importing => (tr("IMPORTING"), Tone::Warn),
        CaptureState::Idle => (tr("READY"), Tone::Ok),
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
    r
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
    widgets::text(ui.painter(), pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("Recorder")));
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
        CaptureState::Idle if !consent => tr("Accept the capture notice on the desktop first"),
        CaptureState::Idle => tr("Start recording"),
        CaptureState::Starting => tr("Starting…"),
        CaptureState::Recording | CaptureState::Paused => tr("Stop recording"),
        CaptureState::Stopping => tr("Saving the session…"),
        CaptureState::Importing => tr("Importing…"),
    };
    let p = ui.painter().clone();
    let y = button.bottom() + 16.0;
    widgets::text_truncated(&p, pos2(card.center().x, y + 8.0), Align2::CENTER_CENTER, &Txt::new(title, 18.0).weight(weight::SEMIBOLD), card.width() - 40.0);
    let sub = match (&c.source, c.state) {
        (Some(source), CaptureState::Recording | CaptureState::Paused) => source.clone(),
        _ if snap.osc.listening => tr("or N4N0 › Record in your Action Menu").into(),
        _ => tr("Saved as a session for Mixer").into(),
    };
    widgets::text_truncated(&p, pos2(card.center().x, y + 34.0), Align2::CENTER_CENTER, &Txt::new(sub, 13.5).color(theme::FG_3), card.width() - 40.0);
    if matches!(c.state, CaptureState::Recording | CaptureState::Paused) {
        let paused = c.state == CaptureState::Paused;
        let b = Button::new(if paused { tr("Resume") } else { tr("Pause") }).icon(if paused { icons::PLAY } else { icons::PAUSE }).size(Size::Md);
        let w = b.desired_width(ui).max(140.0);
        if b.width(w).show_at(ui, Rect::from_min_size(pos2(card.center().x - w / 2.0, card.bottom() - 64.0), vec2(w, 40.0))).clicked() {
            ctl.pause_capture(!paused);
        }
    } else if c.state == CaptureState::Idle {
        // The two 3D pictures, a little of their length: as the Capture page has them.
        use orbiters_core::settings::{Photo3dLength, PictureMode};
        let enabled = consent && snap.vrchat.running;
        let people = Button::new(tr_ctx("3D picture", "People")).icon(icons::USERS).variant(Variant::Secondary).size(Size::Md).enabled(enabled);
        let world = Button::new(tr_ctx("3D picture", "World")).icon(icons::GLOBE_HEMISPHERE_WEST).variant(Variant::Secondary).size(Size::Md).enabled(enabled);
        let (pw, ww) = (people.desired_width(ui).max(120.0), world.desired_width(ui).max(120.0));
        let seg_w = 150.0;
        let left = card.center().x - (pw + 8.0 + ww + 12.0 + seg_w) / 2.0;
        let y = card.bottom() - 64.0;
        if people.width(pw).show_at(ui, Rect::from_min_size(pos2(left, y), vec2(pw, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::Photo3DOf { mode: PictureMode::People });
        }
        if world.width(ww).show_at(ui, Rect::from_min_size(pos2(left + pw + 8.0, y), vec2(ww, 40.0))).clicked() {
            ctl.n4n0(N4n0Command::Photo3DOf { mode: PictureMode::World });
        }
        let mut length = snap.settings.photo_3d;
        let lengths = [SegOption::new(Photo3dLength::Quick, "1 s"), SegOption::new(Photo3dLength::Standard, "4 s"), SegOption::new(Photo3dLength::Full, "8 s")];
        let mut seg = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(left + pw + 8.0 + ww + 12.0, y), vec2(seg_w, 40.0))).id_salt("overlay-photo-length"));
        if widgets::segmented(&mut seg, "overlay-photo-length", &mut length, &lengths, false) {
            let mut settings = snap.settings.clone();
            settings.photo_3d = length;
            ctl.save_changes(&snap.settings, &settings);
        }
    }
}

fn streams(ui: &mut Ui, snap: &Snapshot, ctl: &Ctl, card: Rect) {
    widgets::card(ui.painter(), card, theme::CARD_RADIUS);
    widgets::text(ui.painter(), pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr("What is recorded")));
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
        ctl.save_changes(&snap.settings, &s);
    }
    if capturing {
        let p = ui.painter();
        widgets::text(
            p,
            pos2(card.right() - 22.0, card.top() + 28.0),
            Align2::RIGHT_CENTER,
            &Txt::new(tr("Switches apply to the next recording"), 12.0).color(theme::FG_4),
        );
    }
}
