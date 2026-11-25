//! Live translation in VR on the Settings page: the same choices and the same status line as the orbit menu's
//! Translation sheet, plus what needs a keyboard there (an AI server's address, the model to ask).

use super::{section, Cx};
use crate::overlay::orbit::panels::{orbiters_notice_both, small_model_advice, status_words, target_name, TranslateFacts, TARGETS};
use egui::{vec2, Sense, Ui};
use orbiters_core::settings::{Settings, TranslateProvider};
use orbiters_i18n::tr;
use orbiters_ui::popup::{self, Item};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size};
use orbiters_ui::icons;

fn wrapped(ui: &mut Ui, text: &str, size: f32, color: egui::Color32, strong: bool) {
    let mut txt = Txt::new(text, size).color(color).line_height(size * 1.35);
    if strong {
        txt = txt.weight(weight::SEMIBOLD);
    }
    let galley = ui.painter().layout_job(txt.job(ui.available_width()));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// The card. Whether the settings changed.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut changed = false;
    let snap = cx.snap;
    section(ui, cx, index, icons::GLOBE, tr("Translation in VR"), Some(tr("Signs and chat in your language, laid over the text where it is.")), |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let t = &mut draft.translate;
        widgets::field(ui, tr("Translation"), Some(tr("Reads the text your headset shows, a few times a second. Off, nothing of it runs.")), 48.0, |ui| {
            changed |= widgets::toggle(ui, "translate-on", &mut t.enabled);
        });
        widgets::field(ui, tr("Into"), Some(tr("The language translations are written in.")), 260.0, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(260.0, 30.0), Sense::hover());
            let app = orbiters_i18n::language().tag().to_string();
            let app_name = format!("{} ({})", target_name(&app), tr("the app's language"));
            let mut names: Vec<String> = vec![app_name];
            names.extend(TARGETS.iter().map(|(_, n)| n.to_string()));
            let items: Vec<Item> = names.iter().map(|n| Item::new(n)).collect();
            let at = match &t.target {
                None => Some(0),
                Some(tag) => TARGETS.iter().position(|(x, _)| x.eq_ignore_ascii_case(tag)).map(|i| i + 1),
            };
            let shown = at.map_or_else(|| names[0].clone(), |i| names[i].clone());
            if let Some(picked) = popup::dropdown(ui, "translate-into", rect, &shown, &items, at, true) {
                let new = if picked == 0 { None } else { Some(TARGETS[picked - 1].0.to_string()) };
                if new != t.target {
                    t.target = new;
                    changed = true;
                }
            }
        });
        widgets::field(ui, tr("Translator"), Some(tr("Who translates what is read.")), 330.0, |ui| {
            let options = [
                SegOption::new(TranslateProvider::ThisComputer, tr("This computer")),
                SegOption::new(TranslateProvider::Address, tr("AI server")),
                SegOption::new(TranslateProvider::Orbiters, tr("Orbiters")),
            ];
            changed |= widgets::segmented(ui, "translate-provider", &mut t.provider, &options, true);
        });
        match t.provider {
            TranslateProvider::ThisComputer => wrapped(ui, tr("An AI app on this computer: LM Studio, Ollama or llama.cpp's server, with a model loaded. Nothing leaves your computer."), 13.0, theme::fg_2(), false),
            TranslateProvider::Address => {
                widgets::field(ui, tr("Address"), Some(tr("An OpenAI-compatible server on your network, such as LM Studio's.")), 260.0, |ui| {
                    let r = widgets::text_field(ui, "translate-server", &mut t.server, "http://192.168.1.20:1234", 260.0, true);
                    if r.lost_focus() {
                        changed = true;
                    }
                });
            }
            TranslateProvider::Orbiters => {
                // Here one agreement covers both places the text is read (verify-r15 B7): VR, and VRChat's window on the desktop, each asked for until then.
                let agreed = t.orbiters_agreed && t.orbiters_agreed_window;
                wrapped(ui, orbiters_notice_both(), 13.0, if agreed { theme::fg_2() } else { theme::fg() }, !agreed);
                if !agreed && Button::new(tr("I agree")).size(Size::Sm).id_salt("translate-agree").show(ui).clicked() {
                    t.orbiters_agreed = true;
                    t.orbiters_agreed_window = true;
                    changed = true;
                }
            }
        }
        // The model, once a server answered with its list.
        let status = &snap.translate;
        if t.provider != TranslateProvider::Orbiters && !status.models.is_empty() {
            widgets::field(ui, tr("Model"), Some(tr("Of those the AI server offers. Automatic picks one made for translating when there is one.")), 260.0, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(260.0, 30.0), Sense::hover());
                let mut names = vec![tr("Automatic").to_string()];
                names.extend(status.models.iter().cloned());
                let items: Vec<Item> = names.iter().map(|n| Item::new(n)).collect();
                let at = if t.model.is_empty() { Some(0) } else { status.models.iter().position(|m| *m == t.model).map(|i| i + 1) };
                let shown = at.map_or_else(|| t.model.clone(), |i| names[i].clone());
                if let Some(picked) = popup::dropdown(ui, "translate-model", rect, &shown, &items, at, true) {
                    t.model = if picked == 0 { String::new() } else { status.models[picked - 1].clone() };
                    changed = true;
                }
            });
        }
        widgets::field(ui, tr("English"), Some(tr("VRChat's menus and most names are in English.")), 48.0, |ui| {
            changed |= widgets::toggle(ui, "translate-english", &mut t.keep_english);
        });
        widgets::separator(ui);
        let facts = TranslateFacts { enabled: t.enabled, provider: t.provider, target: t.target.clone().unwrap_or_else(|| orbiters_i18n::language().tag().to_string()), keep_english: t.keep_english, agreed: t.orbiters_agreed, status: (**status).clone() };
        wrapped(ui, &status_words(&facts), 14.0, theme::fg(), true);
        if let Some(advice) = small_model_advice(&facts) {
            wrapped(ui, &advice, 13.0, theme::fg_2(), false);
        }
        if let crate::state::TranslateState::Problem { detail: Some(detail), .. } = &status.state {
            wrapped(ui, detail, 12.0, theme::fg_3(), false);
        }
        if t.enabled && status.look_ms.is_some() {
            let ms = status.look_ms.unwrap_or(0);
            wrapped(ui, &orbiters_i18n::tr!("A look costs {ms} ms of one processor core.", ms = ms), 12.0, theme::fg_3(), false);
        }
        wrapped(ui, tr("In VR: All tools, Translation."), 12.0, theme::fg_3(), false);
    });
    changed
}
