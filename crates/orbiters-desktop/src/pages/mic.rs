//! "Test microphone": hear a microphone on this PC's output and see its level (settings, and Orb's "hear yourself").

use super::Job;
use egui::{pos2, vec2, Align2, Rect, Sense, Ui};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button};

/// A mic test opening its devices: for which microphone, and the monitor or why it failed.
type Starting = (Option<String>, Job<Result<orbiters_sound::MicMonitor, String>>);

/// The mic test: the chosen microphone played on this PC's output, with its level.
#[derive(Default)]
pub struct MicTest {
    running: Option<(Option<String>, orbiters_sound::MicMonitor)>,
    starting: Option<Starting>,
    /// Shown level, 0..1 (falls back smoothly).
    level: f32,
    error: Option<String>,
}

impl MicTest {
    pub fn start(&mut self, ctx: &egui::Context, device: Option<String>) {
        self.running = None;
        self.error = None;
        let d = device.clone();
        self.starting = Some((device, Job::spawn(ctx, move || orbiters_sound::MicMonitor::start(d).map_err(|e| format!("{e:#}")))));
    }

    pub fn active(&self) -> bool {
        self.running.is_some() || self.starting.is_some()
    }
}

/// "Test microphone": hear the chosen microphone and see its level.
pub fn mic_test(ui: &mut Ui, test: &mut MicTest, device: &str) {
    let ctx = ui.ctx().clone();
    let wanted = (!device.is_empty()).then(|| device.to_string());
    if let Some((d, job)) = test.starting.as_mut() {
        if let Some(result) = job.take() {
            match result {
                Ok(monitor) => test.running = Some((d.clone(), monitor)),
                Err(e) => test.error = Some(e),
            }
            test.starting = None;
        }
    }
    // A running test follows the microphone chosen above.
    if test.running.as_ref().is_some_and(|(d, _)| *d != wanted) {
        test.start(&ctx, wanted.clone());
    }
    let active = test.active();
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let bw = 188.0;
    let button = Button::new(if active { "Stop test" } else { "Test microphone" }).icon(if active { icons::X } else { icons::MICROPHONE }).width(bw);
    if button.show_at(ui, Rect::from_min_size(row.min, vec2(bw, 40.0))).clicked() {
        if active {
            *test = MicTest::default();
        } else {
            test.start(&ctx, wanted);
        }
    }
    // Level: the loudest sample in dB, −60…0 dBFS across the meter, falling back at 1.5 meters a second.
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    let peak = test.running.as_ref().map(|(_, m)| m.take_level()).unwrap_or(0.0);
    let target = ((20.0 * peak.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
    test.level = target.max(test.level - 1.5 * dt);
    if !active {
        test.level = 0.0;
    }
    let meter = Rect::from_min_max(pos2(row.left() + bw + 16.0, row.center().y - 4.0), pos2(row.right(), row.center().y + 4.0));
    let p = ui.painter();
    p.rect_filled(meter, theme::radius(4.0), theme::white(0.07));
    if test.level > 0.005 {
        let fill = Rect::from_min_size(meter.min, vec2((meter.width() * test.level).max(8.0), meter.height()));
        p.rect_filled(fill, theme::radius(4.0), if peak > 0.98 { theme::DANGER } else { theme::ACCENT });
    }
    let (note, color) = match (&test.error, active) {
        (Some(e), _) => (format!("The microphone did not start: {e}"), theme::DANGER),
        (None, true) => ("You hear your microphone on this PC's sound output: wear headphones to avoid feedback.".to_string(), theme::FG_3),
        (None, false) => ("Hear yourself to check the microphone and its level.".to_string(), theme::FG_3),
    };
    let (caption, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    widgets::text_truncated(ui.painter(), pos2(caption.left(), caption.center().y), Align2::LEFT_CENTER, &Txt::new(note, 11.5).color(color), caption.width());
    if active {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}
