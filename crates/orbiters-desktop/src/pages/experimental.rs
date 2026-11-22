//! Settings, Experimental: what the server says this account may try. For now that is the VRChat bridge (`crate::bridge`),
//! which an account without it never sees a trace of: the section is drawn only when the service says the server said yes.

use super::{section, Cx};
use crate::bridge::clients::{Change, Client, ClientStatus, Old, Registration};
use crate::bridge::person::{Paused, HOTKEY};
use crate::bridge::Command;
use egui::{pos2, vec2, Sense, Ui};
use orbiters_core::settings::ExperimentalSettings;
use orbiters_i18n::{mark, tr};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

/// How many of the bridge's last actions show.
const BRIDGE_LINES: usize = 12;

/// What VRChat's Terms say of this, plainly, right where it is turned on. Their Effective Date was October 23, 2026.
const TERMS: &str = mark("VRChat's Terms of Service forbid using the platform with \"automated means\" not provided by VRChat, or \"in a manner inconsistent with individual human usage\" (section 13.2). This bridge is that. VRChat can suspend or terminate your account for it. Nothing here hides it from VRChat.");

#[derive(Default)]
pub struct State {
    /// Which copy button was just pressed, and when.
    copied: Option<(&'static str, f64)>,
    /// The visit of the page whose look at the agents was asked for.
    checked: Option<u64>,
}

/// A button that copies `text`, and says so for a moment.
fn copy_button(ui: &mut Ui, state: &mut State, key: &'static str, label: &str, text: &str) {
    let now = ui.input(|i| i.time);
    let just = state.copied.is_some_and(|(k, at)| k == key && now - at < 1.5);
    let shown = if just { tr("Copied") } else { label };
    if Button::new(shown).icon(if just { icons::CHECK } else { icons::COPY }).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
        ui.ctx().copy_text(text.to_string());
        state.copied = Some((key, now));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1600));
    }
}

/// What a client's row says, and which buttons it has.
struct Row {
    text: String,
    color: egui::Color32,
    /// The main button's words, when it has one.
    add: Option<String>,
    remove: bool,
}

/// A client's state in words. Only what the client said, and only buttons that can work.
fn row_of(status: &ClientStatus, listening: bool) -> Row {
    let name = status.client.name();
    let row = |text: String, color, add: Option<String>, remove| Row { text, color, add, remove };
    let update = || Some(tr("Update").to_string());
    match &status.registration {
        Registration::Unknown => row(tr("Looking…").into(), theme::FG_3, None, false),
        Registration::NotInstalled => row(tr("Not installed on this computer.").into(), theme::FG_3, None, false),
        Registration::NotAdded if listening => row(tr("Not added yet. New sessions get it the moment you add it.").into(), theme::FG_2, Some(tr!("Add to {name}", name = name)), false),
        Registration::NotAdded => row(tr("Not added.").into(), theme::FG_3, None, false),
        Registration::Added if listening => row(tr("Added. New sessions have it.").into(), theme::OK, None, true),
        Registration::Added => row(tr("Added. It answers while the bridge is on.").into(), theme::FG_2, None, true),
        Registration::Outdated(Old::Token) => row(tr("Added with an old token, so it cannot connect.").into(), theme::WARN, update(), true),
        Registration::Outdated(Old::Address) => row(tr("Added with an old address, so it cannot connect.").into(), theme::WARN, update(), true),
        Registration::Outdated(Old::Everything) => row(tr("Added with an old address and token, so it cannot connect.").into(), theme::WARN, update(), true),
        Registration::NotConnecting(why) => row(tr!("Added, and it says it cannot connect: {why}", why = why), theme::WARN, update(), true),
        Registration::Unreadable(why) => row(tr!("It did not say what it has: {why}", why = why), theme::WARN, None, false),
    }
}

/// One agent's row: what it has, its one click, and the way back.
fn client_row(ui: &mut Ui, cx: &Cx, status: &ClientStatus, listening: bool) {
    let row = row_of(status, listening);
    let width = ui.available_width();
    let text_width = width - 240.0;
    let said = ui.painter().layout_job(Txt::new(row.text, 12.5).color(row.color).line_height(18.0).job(text_width));
    let note_color = if status.failed { theme::WARN } else { theme::FG_3 };
    let noted = status.note.as_deref().map(|n| ui.painter().layout_job(Txt::new(n, 12.0).color(note_color).line_height(17.0).job(text_width)));
    let body = 20.0 + said.size().y + noted.as_ref().map_or(0.0, |n| n.size().y + 2.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, body.max(40.0) + 20.0), Sense::hover());
    let mut y = rect.center().y - body * 0.5;
    widgets::text(ui.painter(), egui::pos2(rect.left(), y + 9.0), egui::Align2::LEFT_CENTER, &Txt::new(status.client.name(), 13.5).weight(weight::MEDIUM));
    y += 20.0;
    let said_height = said.size().y;
    ui.painter().galley(egui::pos2(rect.left(), y), said, row.color);
    y += said_height + 2.0;
    if let Some(noted) = noted {
        ui.painter().galley(egui::pos2(rect.left(), y), noted, note_color);
    }
    let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if status.busy {
        Button::new(tr("Working…")).variant(Variant::Secondary).size(Size::Sm).enabled(false).show(&mut buttons);
        return;
    }
    let command = |change| Command::Client { client: status.client, change };
    if row.remove && Button::new(tr("Remove")).variant(Variant::Ghost).size(Size::Sm).show(&mut buttons).clicked() {
        cx.ctl.bridge_command(command(Change::Remove));
    }
    if row.remove && row.add.is_some() {
        buttons.add_space(8.0);
    }
    if let Some(add) = row.add {
        if Button::new(&add).variant(Variant::Primary).size(Size::Sm).show(&mut buttons).clicked() {
            cx.ctl.bridge_command(command(Change::Add));
        }
    }
}

/// The agents, in a fixed order.
fn clients(status: &crate::bridge::Status) -> Vec<&ClientStatus> {
    [Client::ClaudeCode, Client::Codex].iter().filter_map(|c| status.clients.iter().find(|s| s.client == *c)).collect()
}

/// What the bridge is doing, in a sentence.
fn status_line(cx: &Cx) -> (String, egui::Color32) {
    let status = &cx.snap.bridge;
    match (&status.paused, &status.client) {
        (Some(Paused::Person { .. }), _) => (tr("Paused: you are using the computer. The agent goes on 30 seconds after you stop.").into(), theme::WARN),
        (Some(Paused::Held), _) => (tr!("Paused until you resume (here, or {hotkey}).", hotkey = HOTKEY), theme::WARN),
        (None, Some(client)) if status.acting => (tr!("{name} is acting now.", name = client.name), theme::OK),
        (None, Some(client)) if client.connected => (tr!("{name} is connected.", name = client.name), theme::OK),
        (None, Some(client)) => (tr!("{name} was here, and is not connected now.", name = client.name), theme::FG_2),
        (None, None) => (tr("Waiting for an MCP client. No client has connected.").into(), theme::FG_2),
    }
}

/// The section; true when `draft` changed.
pub fn show(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State, draft: &mut ExperimentalSettings) -> bool {
    let mut changed = false;
    let status = &cx.snap.bridge;
    section(ui, cx, index, icons::FLASK, tr("Experimental"), Some(tr("Offered to this account by Orbiters, and not finished")), |ui| {
        let note = tr("Lets an agent such as Claude Code start VRChat in desktop mode, go from private instance to private instance, look around and take 3D pictures, through a service on this computer only. Your own keyboard or mouse pauses it.");
        widgets::field(ui, tr("VRChat bridge"), Some(note), 48.0, |ui| {
            changed |= widgets::toggle(ui, "vrchat-bridge", &mut draft.vrchat_bridge);
        });

        // The risk, in a filled note: on or off, this is what the switch above means.
        let width = ui.available_width();
        let text = ui.painter().layout_job(Txt::new(tr(TERMS), 12.0).color(theme::FG_2).line_height(18.0).job(width - 70.0));
        let (rect, _) = ui.allocate_exact_size(vec2(width, text.size().y + 24.0), Sense::hover());
        widgets::frame(ui.painter(), rect, 12.0, theme::WARN.gamma_multiply(0.09));
        widgets::icon(ui.painter(), egui::pos2(rect.left() + 26.0, rect.top() + 21.0), icons::WARNING, 17.0, theme::WARN);
        ui.painter().galley(egui::pos2(rect.left() + 50.0, rect.top() + 12.0), text, theme::FG_2);
        ui.add_space(12.0);

        // What the agents have is asked once a visit: it may have changed in a terminal.
        if state.checked != Some(cx.visit) {
            state.checked = Some(cx.visit);
            cx.ctl.bridge_command(Command::CheckClients);
        }

        if let Some(problem) = &status.problem {
            widgets::field(ui, tr("It stays closed"), Some(problem), 0.0, |_| {});
        }
        if !status.listening {
            // Off: an agent that has it can still be cleaned up, and nothing is offered that could not answer.
            for client in clients(status) {
                if matches!(client.registration, Registration::Added | Registration::Outdated(_) | Registration::NotConnecting(_)) {
                    client_row(ui, cx, client, false);
                    widgets::separator(ui);
                }
            }
            return;
        }

        // The way back, first and hard to miss.
        let stop = Button::new(tr("Stop: take back control")).icon(icons::PROHIBIT).variant(Variant::Stop).size(Size::Lg).width(width).show(ui);
        if stop.clicked() {
            cx.ctl.bridge_command(Command::Stop);
        }
        ui.add_space(8.0);

        // What it is doing now, with the way to hold it.
        let (line, color) = status_line(cx);
        let hold = status.paused == Some(Paused::Held);
        let shown = ui.painter().layout_job(Txt::new(line, 13.5).weight(weight::MEDIUM).color(color).job(width - 180.0));
        let (row, _) = ui.allocate_exact_size(vec2(width, shown.size().y.max(36.0) + 16.0), Sense::hover());
        ui.painter().circle_filled(egui::pos2(row.left() + 5.0, row.center().y), 4.0, color);
        ui.painter().galley(egui::pos2(row.left() + 20.0, row.center().y - shown.size().y * 0.5), shown, color);
        let mut controls = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
        let label = if hold { tr("Resume") } else { tr("Pause") };
        if Button::new(label).icon(if hold { icons::PLAY } else { icons::PAUSE }).variant(Variant::Secondary).size(Size::Sm).show(&mut controls).clicked() {
            cx.ctl.bridge_command(if hold { Command::Resume } else { Command::Pause });
        }
        ui.add_space(8.0);
        widgets::separator(ui);

        // One click, per agent.
        for client in clients(status) {
            client_row(ui, cx, client, true);
            widgets::separator(ui);
        }

        let address = status.address.clone().unwrap_or_default();
        let token = status.token.clone().unwrap_or_default();
        widgets::field(ui, tr("Address"), Some(&address), 100.0, |ui| copy_button(ui, state, "address", tr("Copy"), &address));
        widgets::separator(ui);
        widgets::field(ui, tr("Token"), Some(tr("For any other MCP client, with the address above. Whoever has it can drive VRChat here, so keep it to yourself. A new one also updates the agents above.")), 240.0, |ui| {
            copy_button(ui, state, "token", tr("Copy"), &token);
            ui.add_space(8.0);
            if Button::new(tr("New token")).icon(icons::ARROW_CLOCKWISE).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                cx.ctl.bridge_command(Command::NewToken);
            }
        });
        widgets::separator(ui);
        let hotkey = if status.hotkey_taken {
            tr!("{hotkey} is taken by another program: your own input and Stop still work.", hotkey = HOTKEY)
        } else {
            tr!("{hotkey} pauses it until pressed again.", hotkey = HOTKEY)
        };
        widgets::field(ui, tr("Taking over"), Some(&tr!("Any key or mouse movement of yours pauses it and releases what it held. {hotkey}", hotkey = hotkey)), 0.0, |_| {});
        widgets::separator(ui);
        ui.add_space(12.0);
        widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 7.0), egui::Align2::LEFT_CENTER, &Txt::new(tr("Recent actions"), 13.5).weight(weight::MEDIUM));
        ui.add_space(24.0);
        // The last actions, newest at the bottom, in the page's own scroll (never a scroll area inside it).
        let mut galleys = Vec::new();
        if status.log.is_empty() {
            galleys.push((ui.painter().layout_job(Txt::new(tr("Nothing yet."), 11.5).mono().color(theme::FG_3).line_height(17.0).job(width - 24.0)), theme::FG_3));
        }
        for line in status.log.iter().skip(status.log.len().saturating_sub(BRIDGE_LINES)) {
            let color = if line.ok { theme::FG_2 } else { theme::WARN };
            // Today's actions: the time says enough.
            let when = super::local_time_unix(line.at);
            let time = when.rsplit(", ").next().unwrap_or(&when);
            let text = format!("{time}  {}", line.text);
            galleys.push((ui.painter().layout_job(Txt::new(text, 11.5).mono().color(color).line_height(17.0).job(width - 24.0)), color));
        }
        let height = galleys.iter().map(|(g, _)| g.size().y).sum::<f32>() + 24.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, theme::radius(12.0), theme::INK_900);
        let mut y = rect.top() + 12.0;
        for (g, color) in galleys {
            let h = g.size().y;
            ui.painter().galley(pos2(rect.left() + 12.0, y), g, color);
            y += h;
        }
        ui.add_space(14.0);
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(registration: Registration) -> ClientStatus {
        ClientStatus { client: Client::Codex, registration, busy: false, note: None, failed: false }
    }

    #[test]
    fn each_state_offers_only_what_can_work() {
        let row = |r, listening| row_of(&status(r), listening);
        let not_installed = row(Registration::NotInstalled, true);
        assert!(not_installed.text.contains("not installed") || not_installed.text.contains("Not installed"));
        assert!(not_installed.add.is_none() && !not_installed.remove);
        assert_eq!(row(Registration::NotAdded, true).add.as_deref(), Some("Add to Codex"));
        assert!(row(Registration::NotAdded, false).add.is_none(), "nothing would answer it");
        let added = row(Registration::Added, true);
        assert!(added.add.is_none() && added.remove);
        for old in [Old::Token, Old::Address, Old::Everything] {
            let outdated = row(Registration::Outdated(old), true);
            assert_eq!(outdated.add.as_deref(), Some("Update"));
            assert!(outdated.remove && outdated.text.contains("old"));
        }
        let failing = row(Registration::NotConnecting("HTTP 401".into()), true);
        assert!(failing.text.contains("HTTP 401") && failing.add.is_some() && failing.remove);
        let unreadable = row(Registration::Unreadable("broken".into()), true);
        assert!(unreadable.text.contains("broken") && unreadable.add.is_none() && !unreadable.remove);
    }
}
