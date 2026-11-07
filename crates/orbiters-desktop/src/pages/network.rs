//! VRChat on the network, where it does not run on this computer: what was found, which one the app talks with, and how
//! to have one show up. Home shows it until a VRChat is connected, Settings always (controller/lan.rs does the work).

use super::{section, Cx};
use crate::state::{LanLink, LanStatus};
use egui::{vec2, Align2, Sense, Ui};
use orbiters_core::lan::{self, FoundVrchat};
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{icons, toast};
use std::net::Ipv4Addr;

/// The address as it is being typed.
#[derive(Default)]
pub struct State {
    typed: String,
}

/// What the sidebar says of VRChat.
pub fn sidebar_status(lan: &LanStatus) -> (String, Tone) {
    let short = || lan.peer_label.as_deref().map(|l| l.trim_start_matches("VRChat on ").trim_start_matches("VRChat at ").to_string()).unwrap_or_default();
    match lan.link {
        LanLink::Off => (tr("OSC off").into(), Tone::Off),
        LanLink::NoNetwork => (tr("No local network").into(), Tone::Warn),
        LanLink::NotAllowed => (tr("Allow the local network").into(), Tone::Warn),
        LanLink::Looking => (tr("Looking on the network").into(), Tone::Off),
        LanLink::Choose => (tr!("{count} found: choose one", count = lan.found.len()), Tone::Warn),
        LanLink::Waiting => (tr!("Waiting for {label}", label = short()), Tone::Warn),
        LanLink::Connected => (short(), Tone::Ok),
    }
}

/// The card's subtitle.
pub fn summary(lan: &LanStatus) -> String {
    let label = lan.peer_label.as_deref().unwrap_or("VRChat");
    match lan.link {
        LanLink::Off => tr("OSC is off: turn on Action Menu control below").into(),
        LanLink::NoNetwork => tr("This Mac is on no local network").into(),
        LanLink::NotAllowed => tr("macOS keeps this app off your local network").into(),
        LanLink::Looking => tr("Looking on your network…").into(),
        LanLink::Choose => tr("Several found: choose one").into(),
        LanLink::Waiting => tr!("Waiting for {label}", label = label),
        LanLink::Connected => match lan.peer {
            Some(ip) if !label.contains(&ip.to_string()) => tr!("Connected to {label} · {ip}", label = label, ip = ip),
            _ => tr!("Connected to {label}", label = label),
        },
    }
}

/// The address typed, if it is one.
fn parse_address(text: &str) -> Option<Ipv4Addr> {
    text.trim().parse().ok()
}

fn describe(found: &FoundVrchat) -> String {
    match found.osc_port {
        Some(port) => tr!("{address} · input port {port}", address = found.ip, port = port),
        None => tr!("{address} · does not answer over the network yet: on a PC, see below", address = found.ip),
    }
}

/// The card: shows what is known and writes the choice into `draft`. Whether it changed anything.
pub fn card(ui: &mut Ui, cx: &mut Cx, index: usize, state: &mut State, draft: &mut Settings) -> bool {
    let snap = cx.snap;
    let lan = &snap.lan;
    let subtitle = summary(lan);
    let mut changed = false;
    let mut look_again = false;
    let mut copied = None;
    let mut refused = None;
    let mut open_privacy = false;
    section(ui, cx, index, icons::BROADCAST, tr("VRChat on your network"), Some(&subtitle), |ui| {
        if lan.link == LanLink::NotAllowed {
            let why = tr("Answer macOS's question if it is on the screen, or turn on Orbiters - N4N0 in System Settings › Privacy & Security › Local Network. VRChat cannot be found or reached until then.");
            widgets::field(ui, tr("Allow the local network"), Some(why), 150.0, |ui| {
                if Button::new(tr("Open Settings")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                    open_privacy = true;
                }
            });
            widgets::separator(ui);
        }
        for found in &lan.found {
            let chosen = lan.peer == Some(found.ip);
            widgets::field(ui, &found.label(), Some(&describe(found)), 120.0, |ui| {
                if chosen {
                    let (text, color) = if lan.link == LanLink::Connected { (tr("Connected"), theme::OK) } else { (tr("Waiting"), theme::WARN) };
                    let (r, _) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::hover());
                    widgets::text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(text, 12.0).color(color));
                } else if Button::new(tr("Connect")).variant(Variant::Primary).size(Size::Sm).id_salt(&found.name).show(ui).clicked() {
                    draft.osc.remote = lan::Wanted { typed: None, name: found.name.clone(), ip: Some(found.ip) };
                    changed = true;
                }
            });
            widgets::separator(ui);
        }
        if let Some(typed) = draft.osc.remote.typed {
            let text = tr!("{typed}: only this address is talked with.", typed = typed);
            widgets::field(ui, tr("Address you typed"), Some(&text), 150.0, |ui| {
                if Button::new(tr("Find it by itself")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                    draft.osc.remote = lan::Wanted::default();
                    changed = true;
                }
            });
            widgets::separator(ui);
        }
        if lan.found.is_empty() || lan.link != LanLink::Connected {
            let standalone = tr("On a Quest, a Pico or an Android phone: same Wi-Fi as this Mac, then in VRChat's Action Menu, Options › OSC › Enabled. It shows up here by itself.");
            widgets::field(ui, tr("VRChat on a headset or a phone"), Some(standalone), 140.0, |ui| {
                if Button::new(tr("Look again")).icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).show(ui).clicked() {
                    look_again = true;
                }
            });
            widgets::separator(ui);
        }
        if let Some(ip) = lan.this_computer {
            let option = lan::launch_option(ip, snap.osc.port.unwrap_or(lan::OSC_PORT));
            let text = tr!("Add this to VRChat's launch options on Steam, then start it, so that it sends here: {option}", option = option);
            widgets::field(ui, tr("VRChat on another PC"), Some(&text), 110.0, |ui| {
                if Button::new(tr("Copy")).icon(icons::COPY).size(Size::Sm).show(ui).clicked() {
                    copied = Some(option.clone());
                }
            });
            widgets::separator(ui);
        }
        widgets::field(ui, tr("Enter its address"), Some(tr("When it is not found by itself (some routers keep devices from finding each other).")), 300.0, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            let can = parse_address(&state.typed).is_some();
            let pressed = Button::new(tr("Connect")).size(Size::Sm).enabled(can).id_salt("connect-typed").show(ui).clicked();
            let field = widgets::text_field(ui, "vrchat-address", &mut state.typed, "192.168.1.23", 180.0, true);
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if pressed || entered {
                match parse_address(&state.typed) {
                    Some(ip) => {
                        draft.osc.remote = lan::Wanted { typed: Some(ip), ..Default::default() };
                        state.typed.clear();
                        changed = true;
                    }
                    None if entered && !state.typed.trim().is_empty() => refused = Some(tr!("{typed} is not an address like 192.168.1.23.", typed = state.typed.trim())),
                    None => {}
                }
            }
        });
    });
    let ctx = ui.ctx().clone();
    if look_again {
        cx.ctl.osc_reconnect();
    }
    if open_privacy {
        // System Settings' page, by the address macOS gives it.
        if let Err(e) = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork").spawn() {
            cx.toasts.push(&ctx, toast::Kind::Error, tr!("System Settings did not open: {error}", error = e));
        }
    }
    if let Some(option) = copied {
        ctx.copy_text(option);
        cx.toasts.push(&ctx, toast::Kind::Success, tr("Copied."));
    }
    if let Some(message) = refused {
        cx.toasts.push(&ctx, toast::Kind::Error, message);
    }
    changed
}

/// [`card`] on a page that has no draft of the settings of its own: what it changes is saved here.
pub fn card_saving(ui: &mut Ui, cx: &mut Cx, index: usize, state: &mut State) {
    let base = cx.snap.settings.clone();
    let mut draft = base.clone();
    if card(ui, cx, index, state, &mut draft) {
        super::form::save(cx.ctl, &base, &draft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(link: LanLink) -> LanStatus {
        LanStatus { link, peer: Some(Ipv4Addr::new(192, 168, 1, 23)), peer_label: Some("VRChat on Quest-3".into()), ..Default::default() }
    }

    #[test]
    fn the_sidebar_says_where_vrchat_is() {
        assert_eq!(sidebar_status(&status(LanLink::Connected)), ("Quest-3".to_string(), Tone::Ok));
        assert_eq!(sidebar_status(&status(LanLink::Waiting)).0, "Waiting for Quest-3");
        assert_eq!(sidebar_status(&LanStatus { link: LanLink::Looking, ..Default::default() }), ("Looking on the network".to_string(), Tone::Off));
        assert_eq!(sidebar_status(&LanStatus { link: LanLink::NotAllowed, ..Default::default() }), ("Allow the local network".to_string(), Tone::Warn));
        let at = LanStatus { link: LanLink::Connected, peer_label: Some("VRChat at 10.0.0.2".into()), ..Default::default() };
        assert_eq!(sidebar_status(&at).0, "10.0.0.2");
    }

    #[test]
    fn the_card_says_what_it_is_connected_to() {
        assert_eq!(summary(&status(LanLink::Connected)), "Connected to VRChat on Quest-3 · 192.168.1.23");
        let at = LanStatus { link: LanLink::Connected, peer: Some(Ipv4Addr::new(10, 0, 0, 2)), peer_label: Some("VRChat at 10.0.0.2".into()), ..Default::default() };
        assert_eq!(summary(&at), "Connected to VRChat at 10.0.0.2", "the address only once");
        assert_eq!(summary(&status(LanLink::Waiting)), "Waiting for VRChat on Quest-3");
    }

    /// What the card says follows the language, with the values where each language puts them, and the sidebar's Tone stays what it was.
    #[test]
    fn the_card_speaks_each_language_with_its_values_in_place() {
        use orbiters_i18n::{in_language, Language};
        let one = FoundVrchat { name: "a".into(), device: None, ip: Ipv4Addr::new(10, 0, 0, 2), query_port: 1, osc_port: None };
        let connected = status(LanLink::Connected);
        let waiting = status(LanLink::Waiting);
        let choose = LanStatus { link: LanLink::Choose, found: vec![one.clone(), one], ..Default::default() };
        for (language, connected_to, waiting_for, found) in [
            (Language::Fr, "Connecté à VRChat on Quest-3 · 192.168.1.23", "En attente de Quest-3", "2 trouvés\u{a0}: choisis-en un"),
            (Language::Ja, "VRChat on Quest-3に接続済み · 192.168.1.23", "Quest-3を待機中", "2件見つかりました：1つ選んでください"),
            (Language::Es, "Conectado a VRChat on Quest-3 · 192.168.1.23", "Esperando a Quest-3", "Se encontraron 2: elige uno"),
            (Language::PtBr, "Conectado a VRChat on Quest-3 · 192.168.1.23", "Aguardando Quest-3", "2 encontrados: escolha um"),
        ] {
            in_language(language, || {
                assert_eq!(summary(&connected), connected_to, "{language:?}");
                assert_eq!(sidebar_status(&waiting), (waiting_for.to_string(), Tone::Warn), "{language:?}");
                assert_eq!(sidebar_status(&choose), (found.to_string(), Tone::Warn), "{language:?}");
                assert_eq!(sidebar_status(&connected).1, Tone::Ok, "{language:?}");
            });
        }
    }

    #[test]
    fn only_an_address_can_be_entered() {
        assert_eq!(parse_address(" 192.168.1.23 "), Some(Ipv4Addr::new(192, 168, 1, 23)));
        for text in ["", "quest", "192.168.1", "192.168.1.300"] {
            assert_eq!(parse_address(text), None, "{text}");
        }
    }
}
