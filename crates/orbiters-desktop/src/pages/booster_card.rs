//! The FPS booster on the Settings page: the same stops, the same words and the same state as the overlay's sheet, in the
//! Settings' palette. Four stops are a segmented control, as they are not a continuous range.

use super::{section, Cx};
use crate::booster::{self, Command};
use egui::{Sense, Ui};
use orbiters_booster::engine::Request;
use orbiters_booster::guided::Item;
use orbiters_booster::trial::Kind;
use orbiters_booster::Level;
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};

#[derive(Default)]
pub struct State {
    details: bool,
    /// Moving to Avatars the first time asks once.
    asking: bool,
}

fn wrapped(ui: &mut Ui, text: &str, size: f32, color: egui::Color32, strong: bool) {
    let mut txt = Txt::new(text, size).color(color).line_height(size * 1.35);
    if strong {
        txt = txt.weight(weight::SEMIBOLD);
    }
    let galley = ui.painter().layout_job(txt.job(ui.available_width()));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

fn button(ui: &mut Ui, label: &str, salt: &str) -> bool {
    Button::new(label).variant(Variant::Ghost).size(Size::Sm).id_salt(salt).show(ui).clicked()
}

/// The card. Whether the settings changed.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State, draft: &mut Settings) -> bool {
    let mut changed = false;
    let snap = cx.snap;
    let status = &snap.booster;
    let ctl = cx.ctl;
    section(ui, cx, index, icons::GAUGE, tr("FPS booster"), Some(tr("Spend what helps your frames. Shows what it really gains.")), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let options: Vec<SegOption<Level>> = Level::ALL.into_iter().map(|level| SegOption::new(level, booster::level_name(level))).collect();
        let mut level = draft.booster.level;
        if widgets::segmented(ui, "booster-level", &mut level, &options, false) {
            if level == Level::Avatars && !draft.booster.avatars_told {
                state.asking = true;
            } else {
                state.asking = false;
                draft.booster.level = level;
                changed = true;
            }
        }
        wrapped(ui, booster::level_line(draft.booster.level), 13.0, theme::FG_2, false);
        if state.asking {
            wrapped(ui, booster::avatars_question(), 13.0, theme::FG, true);
            ui.horizontal(|ui| {
                if Button::new(tr("Show me how")).size(Size::Sm).id_salt("booster-show").show(ui).clicked() {
                    draft.booster.level = Level::Avatars;
                    draft.booster.avatars_told = true;
                    state.asking = false;
                    changed = true;
                }
                if button(ui, tr("Stay on Picture"), "booster-stay") {
                    draft.booster.level = Level::Picture;
                    state.asking = false;
                    changed = true;
                }
            });
        }
        widgets::separator(ui);
        wrapped(ui, booster::status_line(snap), 14.0, theme::FG, true);

        // Before and after, only from a comparison that ran to its end.
        let report = &status.overlay.report;
        match report.comparisons.first() {
            Some(comparison) => {
                let said = booster::before_after(comparison);
                wrapped(ui, said.title, 12.0, theme::FG_3, false);
                wrapped(ui, &said.figures, 18.0, theme::FG, true);
                wrapped(ui, said.said, 12.0, theme::FG_2, false);
                wrapped(ui, &said.filled, 12.0, theme::FG_3, false);
            }
            None => {
                if let Some(now) = booster::right_now(snap) {
                    wrapped(ui, &now, 13.0, theme::FG_2, false);
                }
            }
        }
        if status.overlay.streamer {
            wrapped(ui, booster::streamer_line(), 12.0, theme::FG_3, false);
        }

        // Step 3's list, one item at a time.
        if draft.booster.level == Level::Avatars {
            widgets::separator(ui);
            if !report.guide.likely {
                wrapped(ui, tr("Nothing here is likely to help right now."), 13.0, theme::FG_2, false);
            }
            if let Some(item) = report.guide.item() {
                wrapped(ui, booster::item_name(item), 13.0, theme::FG, true);
                wrapped(ui, &booster::item_now(item, snap), 12.0, theme::FG_3, false);
                guide_buttons(ui, ctl, item, report.guide.measuring.is_some());
            }
            if let Some((_, left)) = report.guide.measuring {
                wrapped(ui, &orbiters_i18n::tr!("Measuring for {seconds} more seconds.", seconds = left.ceil() as u32), 12.0, theme::FG_3, false);
            }
            for m in report.guide.results.iter().rev().take(4) {
                wrapped(
                    ui,
                    &format!(
                        "{} {}",
                        booster::put_back_words(&orbiters_booster::guided::ToPutBack { item: m.item, was: None }).split(':').next().unwrap_or_default(),
                        booster::measured_words(m)
                    ),
                    12.0,
                    theme::FG_2,
                    false,
                );
            }
        }
        // Leaving step 3 shows what the person changed in VRChat, until it is back.
        if draft.booster.level != Level::Avatars && !status.to_put_back.is_empty() {
            widgets::separator(ui);
            wrapped(ui, tr("Put these back in VRChat's menu if you want them as they were:"), 13.0, theme::FG, true);
            for item in &status.to_put_back {
                wrapped(ui, &booster::put_back_words(item), 12.0, theme::FG_2, false);
            }
            if button(ui, tr("I've put them back"), "booster-put-back-done") {
                ctl.booster(Command::PutBackDone);
            }
        }

        if button(ui, if state.details { tr("Hide details") } else { tr("Details") }, "booster-details") {
            state.details = !state.details;
        }
        if state.details {
            details(ui, cx);
        }
    });
    changed
}

fn guide_buttons(ui: &mut Ui, ctl: &crate::backend::Ctl, item: Item, measuring: bool) {
    ui.horizontal(|ui| {
        if !measuring && Button::new(tr("Done")).size(Size::Sm).id_salt("booster-done").show(ui).clicked() {
            ctl.booster(Command::GuidedDone(item));
        }
        if button(ui, tr("Previous"), "booster-previous") {
            ctl.booster(Command::Overlay(Request::GuidedBack));
        }
        if button(ui, tr("Next"), "booster-next") {
            ctl.booster(Command::Overlay(Request::GuidedNext));
        }
    });
}

fn details(ui: &mut Ui, cx: &Cx) {
    let snap = cx.snap;
    let ctl = cx.ctl;
    let status = &snap.booster;
    let changed = booster::changed_now(snap);
    if changed.is_empty() {
        wrapped(ui, tr("Nothing is changed right now."), 12.0, theme::FG_3, false);
    }
    for line in &changed {
        wrapped(ui, line, 12.0, theme::FG_2, false);
    }
    if let Some(times) = booster::times_words(snap) {
        wrapped(ui, &times, 12.0, theme::FG_3, false);
    }
    ui.horizontal_wrapped(|ui| {
        if button(ui, tr("Test sharpness anyway"), "booster-test-sharpness") {
            ctl.booster(Command::Overlay(Request::TestAnyway(Kind::Sharpness)));
        }
        if button(ui, tr("Test lowering programs"), "booster-test-programs") {
            ctl.booster(Command::Overlay(Request::TestAnyway(Kind::Programs)));
        }
        if status.overlay.panel_3d && button(ui, tr("Test lighter 3D panels"), "booster-test-panels") {
            ctl.booster(Command::Overlay(Request::TestAnyway(Kind::PanelQuality)));
        }
        if snap.reconstruction.is_some() && snap.settings.booster.level >= Level::Picture && button(ui, tr("Test 3D pictures waiting"), "booster-test-hold") {
            ctl.booster(Command::Overlay(Request::TestAnyway(Kind::ReconstructionHold)));
        }
        if button(ui, tr("Put everything back"), "booster-put-back") {
            ctl.booster(Command::PutBack);
        }
        if button(ui, tr("Open the log folder"), "booster-logs") {
            ctl.booster(Command::OpenLogs);
        }
    });
    if status.test_tools {
        wrapped(ui, tr("For testing"), 12.0, theme::FG_3, true);
        ui.horizontal_wrapped(|ui| {
            let holding = status.overlay.sharpness.is_some();
            if button(ui, if holding { tr("Let go of the test change") } else { tr("Hold a test change") }, "booster-hold") {
                ctl.booster(Command::Overlay(Request::HoldTestChange(!holding)));
            }
            if button(ui, tr("Simulate crash"), "booster-crash") {
                ctl.booster(Command::Overlay(Request::SimulateCrash));
            }
            if button(ui, if status.test_load { tr("Stop the test load") } else { tr("Start a test load") }, "booster-load") {
                ctl.booster(Command::TestLoad(!status.test_load));
            }
            if button(ui, tr("Write a test report"), "booster-report") {
                ctl.booster(Command::WriteTestReport);
            }
        });
        if let Some(path) = &status.test_report {
            wrapped(ui, &path.display().to_string(), 12.0, theme::FG_3, false);
        }
    }
}
