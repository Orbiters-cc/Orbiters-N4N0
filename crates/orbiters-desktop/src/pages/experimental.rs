//! Settings, Experimental: what the server says this account may try. For now that is the VRChat bridge (`crate::bridge`),
//! which an account without it never sees a trace of: the section is drawn only when the service says the server said yes.

use super::{section, Cx};
use crate::bridge::person::{Paused, HOTKEY};
use crate::bridge::Command;
use egui::{vec2, Sense, Ui};
use orbiters_core::settings::ExperimentalSettings;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

/// What VRChat's Terms say of this, plainly, right where it is turned on. Their Effective Date was October 23, 2026.
const TERMS: &str = "VRChat's Terms of Service forbid using the platform with \"automated means\" not provided by VRChat, or \"in a manner inconsistent with individual human usage\" (section 13.2). This bridge is that. VRChat can suspend or terminate your account for it. Nothing here hides it from VRChat.";

#[derive(Default)]
pub struct State {
    /// Which copy button was just pressed, and when.
    copied: Option<(&'static str, f64)>,
}

/// A button that copies `text`, and says so for a moment.
fn copy_button(ui: &mut Ui, state: &mut State, key: &'static str, label: &str, text: &str) {
    let now = ui.input(|i| i.time);
    let just = state.copied.is_some_and(|(k, at)| k == key && now - at < 1.5);
    let shown = if just { "Copied" } else { label };
    if Button::new(shown).icon(if just { icons::CHECK } else { icons::COPY }).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
        ui.ctx().copy_text(text.to_string());
        state.copied = Some((key, now));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1600));
    }
}

/// Claude Code's command for this address and token.
pub fn claude_command(address: &str, token: &str) -> String {
    format!("claude mcp add --transport http orbiters-vrchat {address} --header \"Authorization: Bearer {token}\"")
}

/// The same as a project's `.mcp.json`.
pub fn claude_json(address: &str, token: &str) -> String {
    let config = serde_json::json!({ "mcpServers": { "orbiters-vrchat": { "type": "http", "url": address, "headers": { "Authorization": format!("Bearer {token}") } } } });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// What the bridge is doing, in a sentence.
fn status_line(cx: &Cx) -> (String, egui::Color32) {
    let status = &cx.snap.bridge;
    match (&status.paused, &status.client) {
        (Some(Paused::Person { .. }), _) => ("Paused: you are using the computer. The agent goes on 30 seconds after you stop.".into(), theme::WARN),
        (Some(Paused::Held), _) => (format!("Paused until you resume (here, or {HOTKEY})."), theme::WARN),
        (None, Some(client)) if status.acting => (format!("{} is acting now.", client.name), theme::OK),
        (None, Some(client)) if client.connected => (format!("{} is connected.", client.name), theme::OK),
        (None, Some(client)) => (format!("{} was here, and is not connected now.", client.name), theme::FG_2),
        (None, None) => ("Waiting for an MCP client. No client has connected.".into(), theme::FG_2),
    }
}

/// The section; true when `draft` changed.
pub fn show(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State, draft: &mut ExperimentalSettings) -> bool {
    let mut changed = false;
    let status = &cx.snap.bridge;
    section(ui, cx, index, icons::FLASK, "Experimental", Some("Offered to this account by Orbiters, and not finished"), |ui| {
        let note = "Lets an agent such as Claude Code start VRChat in desktop mode, go from private instance to private instance, look around and take 3D pictures, through a service on this computer only. Your own keyboard or mouse pauses it.";
        widgets::field(ui, "VRChat bridge", Some(note), 48.0, |ui| {
            changed |= widgets::toggle(ui, "vrchat-bridge", &mut draft.vrchat_bridge);
        });

        // The risk, in a filled note: on or off, this is what the switch above means.
        let width = ui.available_width();
        let text = ui.painter().layout_job(Txt::new(TERMS, 12.0).color(theme::FG_2).line_height(18.0).job(width - 70.0));
        let (rect, _) = ui.allocate_exact_size(vec2(width, text.size().y + 24.0), Sense::hover());
        widgets::frame(ui.painter(), rect, 12.0, theme::WARN.gamma_multiply(0.09));
        widgets::icon(ui.painter(), egui::pos2(rect.left() + 26.0, rect.top() + 21.0), icons::WARNING, 17.0, theme::WARN);
        ui.painter().galley(egui::pos2(rect.left() + 50.0, rect.top() + 12.0), text, theme::FG_2);
        ui.add_space(12.0);

        if let Some(problem) = &status.problem {
            widgets::field(ui, "It stays closed", Some(problem), 0.0, |_| {});
        }
        if !status.listening {
            return;
        }

        // The way back, first and hard to miss.
        let stop = Button::new("Stop: take back control").icon(icons::PROHIBIT).variant(Variant::Danger).size(Size::Lg).width(width).show(ui);
        if stop.clicked() {
            cx.ctl.bridge_command(Command::Stop);
        }
        ui.add_space(4.0);

        let (line, color) = status_line(cx);
        let hold = status.paused == Some(Paused::Held);
        widgets::field(ui, "Now", None, 140.0, |ui| {
            let label = if hold { "Resume" } else { "Pause" };
            if Button::new(label).icon(if hold { icons::PLAY } else { icons::PAUSE }).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                cx.ctl.bridge_command(if hold { Command::Resume } else { Command::Pause });
            }
        });
        let shown = ui.painter().layout_job(Txt::new(line, 12.5).color(color).line_height(18.0).job(width));
        let (rect, _) = ui.allocate_exact_size(shown.size(), Sense::hover());
        ui.painter().galley(rect.min, shown, color);
        ui.add_space(14.0);
        widgets::separator(ui);

        let address = status.address.clone().unwrap_or_default();
        let token = status.token.clone().unwrap_or_default();
        widgets::field(ui, "Address", Some(&address), 100.0, |ui| copy_button(ui, state, "address", "Copy", &address));
        widgets::separator(ui);
        widgets::field(ui, "Token", Some("What a client must present. Whoever has it can drive VRChat here, so keep it to yourself."), 240.0, |ui| {
            copy_button(ui, state, "token", "Copy", &token);
            ui.add_space(8.0);
            if Button::new("New token").icon(icons::ARROW_CLOCKWISE).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                cx.ctl.bridge_command(Command::NewToken);
            }
        });
        widgets::separator(ui);
        widgets::field(
            ui,
            "For Claude Code",
            Some("The command to run once, or the same for a project's .mcp.json. Then paste docs/vrchat-bridge.md's prompt, or use the prompt world_tour."),
            240.0,
            |ui| {
                copy_button(ui, state, "json", "Copy .mcp.json", &claude_json(&address, &token));
                ui.add_space(8.0);
                copy_button(ui, state, "command", "Copy command", &claude_command(&address, &token));
            },
        );
        widgets::separator(ui);
        let hotkey = if status.hotkey_taken {
            format!("{HOTKEY} is taken by another program: your own input and Stop still work.")
        } else {
            format!("{HOTKEY} pauses it until pressed again.")
        };
        widgets::field(ui, "Taking over", Some(&format!("Any key or mouse movement of yours pauses it and releases what it held. {hotkey}")), 0.0, |_| {});
        widgets::separator(ui);
        ui.add_space(12.0);
        widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 7.0), egui::Align2::LEFT_CENTER, &Txt::new("Recent actions", 13.5).weight(weight::MEDIUM));
        ui.add_space(24.0);
        let height = 170.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, theme::radius(12.0), theme::INK_900);
        let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)).id_salt("bridge-log"));
        egui::ScrollArea::vertical().id_salt("bridge-log-scroll").stick_to_bottom(true).auto_shrink([false, false]).show(&mut inside, |ui| {
            if status.log.is_empty() {
                let g = ui.painter().layout_job(Txt::new("Nothing yet.", 11.5).mono().color(theme::FG_3).line_height(17.0).job(width - 24.0));
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, theme::FG_3);
            }
            for line in &status.log {
                let color = if line.ok { theme::FG_2 } else { theme::WARN };
                let text = format!("{}  {}", super::local_time_unix(line.at), line.text);
                let g = ui.painter().layout_job(Txt::new(text, 11.5).mono().color(color).line_height(17.0).job(width - 24.0));
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, color);
            }
        });
        ui.add_space(14.0);
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_config_has_the_address_and_token() {
        let command = claude_command("http://127.0.0.1:47615/mcp", "abc");
        assert_eq!(command, "claude mcp add --transport http orbiters-vrchat http://127.0.0.1:47615/mcp --header \"Authorization: Bearer abc\"");
        let json: serde_json::Value = serde_json::from_str(&claude_json("http://127.0.0.1:47615/mcp", "abc")).unwrap();
        let server = &json["mcpServers"]["orbiters-vrchat"];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:47615/mcp");
        assert_eq!(server["headers"]["Authorization"], "Bearer abc");
    }
}
