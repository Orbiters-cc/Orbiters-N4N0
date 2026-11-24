//! Settings › Beta usage statistics: the performance self-check's hint and the Performance report (`crate::perf`). The
//! report is made only when pressed, shown line by line, saved, and sent only with Send.

use crate::backend::Ctl;
use crate::perf::Command;
use crate::state::Snapshot;
use egui::{vec2, Sense, Ui, UiBuilder};
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

pub fn show(ui: &mut Ui, ctl: &Ctl, snap: &Snapshot) {
    let report = snap.perf.report.as_ref();
    let making = report.is_some_and(|r| r.making);
    widgets::field(
        ui,
        tr("Performance report"),
        Some(tr("If VRChat or your computer lags with N4N0, this puts together what N4N0 measured over the last hour with your computer's specifications and the settings that matter. You see it before anything is sent, and it is kept on this computer.")),
        180.0,
        |ui| {
            let label = if making { tr("Making it…") } else { tr("Make a report") };
            if Button::new(label).icon(icons::GAUGE).size(Size::Sm).enabled(!making).show(ui).clicked() {
                ctl.perf(Command::Make);
            }
        },
    );
    if let Some(hint) = &snap.perf.hint {
        let (title, advice) = hint.words();
        widgets::field(ui, &title, Some(&advice), 180.0, |ui| {
            if Button::new(tr("Don't tell me again")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                ctl.perf(Command::Mute(hint.key.clone()));
            }
        });
    }
    let Some(report) = report.filter(|r| !r.making) else { return };
    // What is in it, as it will be sent.
    let width = ui.available_width();
    let text = report.summary.join("\n");
    let galley = ui.painter().layout_job(Txt::new(text, 12.0).color(theme::FG_2).line_height(18.0).job(width - 24.0));
    let (r, _) = ui.allocate_exact_size(vec2(width, galley.size().y + 24.0), Sense::hover());
    ui.painter().rect_filled(r, theme::radius(12.0), theme::INK_900);
    ui.painter().galley(r.min + vec2(12.0, 12.0), galley, theme::FG_2);
    ui.add_space(8.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let mut bu = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
    let send_label = match (report.sending, report.sent) {
        (true, _) => tr("Sending…"),
        (false, Some(true)) => tr("Sent: thank you"),
        (false, Some(false)) => tr("Send again"),
        (false, None) => tr("Send to Orbiters"),
    };
    if Button::new(send_label).icon(icons::PAPER_PLANE_TILT).variant(Variant::Primary).size(Size::Sm).enabled(!report.sending && report.sent != Some(true)).show(&mut bu).clicked() {
        ctl.perf(Command::Send);
    }
    if let Some(path) = &report.path {
        if Button::new(tr("Show the file")).icon(icons::FOLDER_OPEN).size(Size::Sm).show(&mut bu).clicked() {
            show_file(path);
        }
    }
    if Button::new(tr("Close")).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).show(&mut bu).clicked() {
        ctl.perf(Command::Close);
    }
    if report.sent == Some(false) {
        let why = tr("It could not be sent. The file is saved: you can give it to whoever asked for it.");
        let g = ui.painter().layout_job(Txt::new(why, 12.0).color(theme::WARN).job(width));
        let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(r.min, g, theme::WARN);
    } else if let Some(path) = &report.path {
        let saved = orbiters_i18n::tr!("Saved as {name} in the logs' folder.", name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let g = ui.painter().layout_job(Txt::new(saved, 12.0).color(theme::FG_3).job(width));
        let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(r.min, g, theme::FG_3);
    }
    ui.add_space(8.0);
}

/// Opens Explorer on the report's folder with the file chosen.
fn show_file(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(target_os = "linux")]
    let _ = crate::linux::open::reveal(path);
    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = path.parent() {
            let _ = crate::controller::open_path(dir);
        }
    }
}
