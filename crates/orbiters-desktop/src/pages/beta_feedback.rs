//! Settings › Beta feedback: the switch of the menu mismatch buttons ([`crate::mismatch`]), the switch of their picture, and the reports kept on
//! this computer: how many, Send reports (to Orbiters), Open folder, Delete all. The app has no feedback page of its own: this card is where a
//! beta tester gives feedback, beside the beta's usage statistics.

use super::{section, Cx, Job};
use egui::{vec2, Align2, Sense, Ui};
use orbiters_core::settings::Settings;
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use std::time::Duration;

#[derive(Default)]
pub struct State {
    /// The reports kept, as last counted (all, not sent), and when.
    counted: Option<((usize, usize), f64)>,
    sending: Option<Job<crate::mismatch::send::Outcome>>,
    /// What the last send or delete did, in words.
    said: Option<(String, egui::Color32)>,
}

/// How many reports are kept, and how many of them are not sent yet.
fn count() -> (usize, usize) {
    let kept = crate::mismatch::kept();
    (kept.len(), kept.iter().filter(|k| !k.sent).count())
}

/// The card. Returns whether the settings changed.
pub fn card(ui: &mut Ui, cx: &Cx, id: usize, state: &mut State, draft: &mut Settings) -> bool {
    let mut apply = false;
    let now = ui.input(|i| i.time);
    if state.counted.is_none_or(|(_, at)| now - at > 2.0) {
        state.counted = Some((count(), now));
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }
    if let Some(outcome) = state.sending.as_mut().and_then(|j| j.poll()).cloned() {
        state.sending = None;
        state.counted = None;
        state.said = Some(if outcome.refused {
            (tr("This Orbiters does not take reports yet. They stay on this computer.").to_string(), theme::WARN)
        } else if outcome.failed > 0 && outcome.sent == 0 {
            (tr("Orbiters could not be reached. The reports stay on this computer; try again later.").to_string(), theme::WARN)
        } else {
            (tr_n("{count} report sent. Thank you!", "{count} reports sent. Thank you!", outcome.sent as u64), theme::OK)
        });
    }
    let (all, unsent) = state.counted.map_or((0, 0), |(c, _)| c);
    section(ui, cx, id, icons::CHAT_CIRCLE_TEXT, tr("Beta feedback"), Some(tr("When N4N0's menus do not match VRChat's")), |ui| {
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Menu mismatch buttons"),
            Some(tr("Shows three buttons, always in view: \"Menu should be off\", \"Wheel should be on\" and \"Quick menu should be on\", in VR on your watch's wrist, on the desktop in the bottom left corner of VRChat's window. Press one when N4N0's menus do not match VRChat's. Each press keeps a report on this computer: the controller buttons and keys used for the menus and what N4N0 decided in the 30 seconds before, and VRChat's log lines about menus and worlds. Never what you type or say, nor anyone's name. Turned off, nothing is recorded.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "mismatch-buttons", &mut draft.mismatch.buttons);
            },
        );
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Add a picture of the game"),
            Some(tr("A small, blurred picture of what VRChat showed at the press goes with the report: the best clue of what its menus did, but it is a picture of your game, and can show the world and the people in it.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "mismatch-picture", &mut draft.mismatch.picture);
            },
        );
        widgets::separator(ui);
        ui.add_space(6.0);
        let words = if all == 0 {
            tr("No reports yet.").to_string()
        } else {
            format!("{} {}", tr_n("{count} report on this computer,", "{count} reports on this computer,", all as u64), tr_n("{count} not sent yet.", "{count} not sent yet.", unsent as u64))
        };
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        widgets::text_truncated(ui.painter(), row.left_center(), Align2::LEFT_CENTER, &Txt::new(&words, 12.5).color(theme::FG_2), row.width());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let sending = state.sending.is_some();
            let send = Button::new(if sending { tr("Sending…") } else { tr("Send reports") }).icon(icons::PAPER_PLANE_TILT).variant(Variant::Primary).size(Size::Sm).enabled(unsent > 0 && !sending);
            if send.show(ui).clicked() {
                let kept = crate::mismatch::kept();
                state.said = None;
                state.sending = Some(Job::spawn(ui.ctx(), move || crate::mismatch::send::unsent(&crate::mismatch::send::api(), &kept)));
            }
            if Button::new(tr("Open folder")).icon(icons::FOLDER_OPEN).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                let dir = crate::mismatch::folder();
                let _ = std::fs::create_dir_all(&dir);
                if let Err(e) = crate::controller::open_path(&dir) {
                    state.said = Some((e.to_string(), theme::WARN));
                }
            }
            if Button::new(tr("Delete all")).icon(icons::TRASH).variant(Variant::Secondary).size(Size::Sm).enabled(all > 0 && !sending).confirm(ui) {
                let gone = crate::mismatch::delete_all();
                state.counted = None;
                state.said = Some((tr_n("{count} report deleted.", "{count} reports deleted.", gone as u64), theme::FG_2));
            }
        });
        if let Some((said, colour)) = &state.said {
            ui.add_space(6.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            widgets::text_truncated(ui.painter(), row.left_center(), Align2::LEFT_CENTER, &Txt::new(said, 12.0).color(*colour), row.width());
        }
        ui.add_space(4.0);
    });
    apply
}
