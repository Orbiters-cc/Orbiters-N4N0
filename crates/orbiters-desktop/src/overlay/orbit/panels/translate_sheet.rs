//! Translation's sheet, from All tools: live translation of the text seen in VR, turned on or off, who translates, into
//! which language, and what it is doing now in one plain line. Laid out as the sheet of settings is (a row's name and
//! what it is about down the left, its choices on the right), so it reads as one family with it.

use super::*;
use crate::state::{TranslateState, TranslateStatus};
use orbiters_core::settings::TranslateProvider;
use orbiters_i18n::{tr, tr_ctx};

const ROW_ON: usize = 0;
const ROW_WHO: usize = 1;
const ROW_INTO: usize = 2;
const ROW_ENGLISH: usize = 3;
const ROW_STATUS: usize = 4;

fn row(i: usize) -> f32 {
    8.0 + i as f32 * 76.0
}
const X: f32 = 276.0;
const HIGH: f32 = 58.0;

/// What can be pressed on this sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TranslatePick {
    On(bool),
    Provider(TranslateProvider),
    /// The next language, or the one before.
    Into(bool),
    KeepEnglish(bool),
    /// Accepts that, with Orbiters translating, the text seen in VR is sent to Orbiters.
    Agree,
}

/// What the sheet draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranslateFacts {
    pub enabled: bool,
    pub provider: TranslateProvider,
    /// The language translated into, a website tag.
    pub target: String,
    pub keep_english: bool,
    pub agreed: bool,
    pub status: TranslateStatus,
}

/// The languages one can translate into: the app's five, then others common around VRChat. Each in its own name.
pub const TARGETS: [(&str, &str); 10] = [("en", "English"), ("fr", "Français"), ("ja", "日本語"), ("es", "Español"), ("pt-BR", "Português (Brasil)"), ("de", "Deutsch"), ("ko", "한국어"), ("zh-Hans", "简体中文"), ("ru", "Русский"), ("it", "Italiano")]; // i18n: not shown (languages in their own names)

/// The language after (or before) `now` in [`TARGETS`].
pub fn target_after(now: &str, after: bool) -> &'static str {
    let at = TARGETS.iter().position(|(t, _)| t.eq_ignore_ascii_case(now)).unwrap_or(0);
    let n = TARGETS.len();
    TARGETS[if after { (at + 1) % n } else { (at + n - 1) % n }].0
}

pub fn target_name(tag: &str) -> &'static str {
    TARGETS.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map_or(TARGETS[0].1, |(_, n)| n)
}

pub(super) fn translate_items(facts: &Facts) -> Vec<Item> {
    let f = &facts.translate;
    let mut out = Vec::new();
    let mut choices = |i: usize, list: Vec<(TranslatePick, &'static str, &'static str, bool)>| {
        let w = (1000.0 - X - 12.0 * (list.len() as f32 - 1.0)) / list.len() as f32;
        for (n, (pick, icon, label, on)) in list.into_iter().enumerate() {
            out.push(Item { action: Action::Set(Pick::Translate(pick)), rect: r(X + n as f32 * (w + 12.0), row(i) + 4.0, w, HIGH), look: Look::Choice { icon, label, on } });
        }
    };
    choices(ROW_ON, vec![(TranslatePick::On(true), icons::GLOBE, tr_ctx("switch", "On"), f.enabled), (TranslatePick::On(false), icons::PROHIBIT, tr_ctx("switch", "Off"), !f.enabled)]);
    choices(
        ROW_WHO,
        vec![
            (TranslatePick::Provider(TranslateProvider::ThisComputer), icons::CPU, tr("This computer"), f.provider == TranslateProvider::ThisComputer),
            (TranslatePick::Provider(TranslateProvider::Address), icons::BROADCAST, tr("AI server"), f.provider == TranslateProvider::Address),
            (TranslatePick::Provider(TranslateProvider::Orbiters), icons::N4N0_MARK, tr("Orbiters"), f.provider == TranslateProvider::Orbiters),
        ],
    );
    choices(ROW_ENGLISH, vec![(TranslatePick::KeepEnglish(true), icons::CHECK, tr("Leave English"), f.keep_english), (TranslatePick::KeepEnglish(false), icons::TEXT_T, tr("Translate it too"), !f.keep_english)]);
    let y = row(ROW_INTO) + 4.0;
    out.push(Item { action: Action::Set(Pick::Translate(TranslatePick::Into(false))), rect: r(X, y + 1.0, 56.0, 56.0), look: Look::Step(false) });
    out.push(Item { action: Action::Idle, rect: r(X + 64.0, y, 1000.0 - X - 128.0, HIGH), look: Look::TranslateTarget });
    out.push(Item { action: Action::Set(Pick::Translate(TranslatePick::Into(true))), rect: r(1000.0 - 56.0, y + 1.0, 56.0, 56.0), look: Look::Step(true) });
    if f.provider == TranslateProvider::Orbiters && !f.agreed {
        out.push(Item { action: Action::Set(Pick::Translate(TranslatePick::Agree)), rect: r(1000.0 - 220.0, row(ROW_STATUS) + 70.0, 220.0, HIGH), look: Look::Choice { icon: icons::CHECK, label: tr("I agree"), on: false } });
    }
    out
}

/// The status in plain words: what it does now, or what it waits for.
pub fn status_words(f: &TranslateFacts) -> String {
    if !f.enabled {
        return tr("Off. Turn it on to see signs and chat in your language.").to_string();
    }
    let s = &f.status;
    match &s.state {
        TranslateState::Off | TranslateState::Starting => tr("Starting…").to_string(),
        TranslateState::Downloading { done, total } => {
            let pct = if *total > 0 { (*done as f64 / *total as f64 * 100.0).round() as u32 } else { 0 };
            orbiters_i18n::tr!("Downloading the text reader: {pct} %", pct = pct)
        }
        TranslateState::DownloadingTranslator { done, total } => {
            let pct = if *total > 0 { (*done as f64 / *total as f64 * 100.0).round() as u32 } else { 0 };
            orbiters_i18n::tr!("No AI app answers here: downloading the built-in translator, {pct} %", pct = pct)
        }
        TranslateState::Looking if s.shown > 0 => orbiters_i18n::tr_n("{count} translation shown", "{count} translations shown", s.shown as u64),
        TranslateState::Looking => tr("Looking for text to translate.").to_string(),
        TranslateState::Translating(n) => {
            let words = orbiters_i18n::tr_n("Translating {count} line…", "Translating {count} lines…", *n as u64);
            match s.waited_s.filter(|w| *w >= 2) {
                Some(w) => orbiters_i18n::tr!("{what} ({seconds} s)", what = words, seconds = w),
                None => words,
            }
        }
        TranslateState::Waking => tr("Waking the translator: the AI app is loading its model.").to_string(),
        TranslateState::Paused => tr("Paused while a menu is open.").to_string(),
        TranslateState::NeedsAgreement => tr("Orbiters translates once you agree below.").to_string(),
        TranslateState::NeedsAddress => tr("Type the AI server's address in Orbiters' Settings, Translation.").to_string(),
        TranslateState::NoLocalTranslator => tr("No AI app answers on this computer. Start LM Studio or Ollama with a model, or choose Orbiters.").to_string(),
        TranslateState::Problem { key, .. } => tr(key).to_string(),
    }
}

/// Advice when the AI server's model is too small to translate signs well.
pub fn small_model_advice(f: &TranslateFacts) -> Option<String> {
    let model = f.status.small_model.as_ref().filter(|_| f.enabled && f.provider != TranslateProvider::Orbiters)?;
    Some(orbiters_i18n::tr!("{model} is a small model and translates signs poorly. TranslateGemma 4B or Qwen3 8B translate better.", model = model))
}

/// The notice shown before Orbiters translates.
pub fn orbiters_notice() -> &'static str {
    tr("With Orbiters, the text you see in VR, other people's chat included, is sent to Orbiters to be translated.")
}

/// The notice of the app's own Settings, which agrees for both: VR, and VRChat's window on the desktop.
pub fn orbiters_notice_both() -> &'static str {
    tr("With Orbiters, the text you see in VR or in VRChat's window, other people's chat included, is sent to Orbiters to be translated.")
}

/// The same where it is said: on the desktop it is VRChat's window that is read (REQUEST15 item 2).
pub fn orbiters_notice_on(desktop: bool) -> &'static str {
    if desktop {
        tr("With Orbiters, the text VRChat's window shows, other people's chat included, is sent to Orbiters to be translated.")
    } else {
        orbiters_notice()
    }
}

pub(super) fn draw_translate(p: &Painter, facts: &Facts) {
    let rows = [
        (ROW_ON, tr("Translation"), tr("Text you see, in your language, where it is")),
        (ROW_WHO, tr("Translator"), tr("Who translates what is read")),
        (ROW_INTO, tr("Into"), tr("The language you read")),
        (ROW_ENGLISH, tr("English"), tr("VRChat's menus and most names")),
        (ROW_STATUS, tr("Now"), tr("What it is doing")),
    ];
    for (i, name, about) in rows {
        let y = row(i);
        p.text(pos2(26.0, y + 15.0), Align2::LEFT_CENTER, name, look::font(20.0, weight::BOLD), look::LABEL);
        let mut job = egui::text::LayoutJob::simple(about.to_string(), look::font(15.0, weight::MEDIUM), look::LABEL, X - 40.0);
        job.wrap.max_rows = 2;
        p.galley(pos2(26.0, y + 28.0), p.layout_job(job), look::LABEL);
    }
    let f = &facts.translate;
    let y = row(ROW_STATUS);
    let mut job = egui::text::LayoutJob::simple(status_words(f), look::font(19.0, weight::SEMIBOLD), look::LABEL, 1000.0 - X);
    job.wrap.max_rows = 2;
    p.galley(pos2(X, y + 6.0), p.layout_job(job), look::LABEL);
    if let Some(advice) = small_model_advice(f) {
        let mut job = egui::text::LayoutJob::simple(advice, look::font(16.0, weight::MEDIUM), look::MUTED, 1000.0 - X);
        job.wrap.max_rows = 2;
        p.galley(pos2(X, y + 66.0), p.layout_job(job), look::MUTED);
    }
    if f.provider == TranslateProvider::Orbiters && !f.agreed {
        let mut job = egui::text::LayoutJob::simple(orbiters_notice_on(facts.desktop).to_string(), look::font(16.0, weight::MEDIUM), look::MUTED, 1000.0 - X - 240.0);
        job.wrap.max_rows = 3;
        p.galley(pos2(X, y + 72.0), p.layout_job(job), look::MUTED);
    }
}

pub(super) fn draw_target(p: &Painter, rect: Rect, facts: &Facts) {
    draw_stepper_name(p, rect, icons::GLOBE, target_name(&facts.translate.target));
}

pub(super) fn translate_caption(action: Action, desktop: bool) -> Option<(String, String)> {
    let (a, b) = match action {
        Action::Translate => (tr("Translation"), tr("Signs and chat in your language, laid over the text where it is.")),
        Action::Set(Pick::Translate(TranslatePick::Provider(TranslateProvider::ThisComputer))) => (tr("This computer"), tr("An AI app here (LM Studio, Ollama): nothing leaves your computer.")),
        Action::Set(Pick::Translate(TranslatePick::Provider(TranslateProvider::Address))) => (tr("AI server"), tr("An AI server on your network, at the address set in Orbiters' Settings.")),
        Action::Set(Pick::Translate(TranslatePick::Provider(TranslateProvider::Orbiters))) => (tr("Orbiters"), orbiters_notice_on(desktop)),
        Action::Set(Pick::Translate(TranslatePick::Into(_))) => (tr("Into"), tr("The language translations are written in.")),
        _ => return None,
    };
    Some((a.to_string(), b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_steps_round() {
        assert_eq!(target_after("en", true), "fr");
        assert_eq!(target_after("en", false), "it");
        assert_eq!(target_after("pt-br", true), "de", "tags match whatever their case");
        assert_eq!(target_after("xx", true), "fr", "an unknown language starts from English");
    }

    #[test]
    fn the_sheet_offers_each_choice_and_asks_before_orbiters() {
        let mut facts = Facts { sheet: Some(Sheet::Translate), ..Facts::default() };
        facts.translate = TranslateFacts { enabled: true, provider: TranslateProvider::Orbiters, target: "fr".into(), keep_english: true, agreed: false, status: TranslateStatus::default() };
        let items = translate_items(&facts);
        let picks: Vec<_> = items.iter().filter_map(|i| if let Action::Set(Pick::Translate(p)) = i.action { Some(p) } else { None }).collect();
        for p in [TranslatePick::On(true), TranslatePick::On(false), TranslatePick::Provider(TranslateProvider::ThisComputer), TranslatePick::Provider(TranslateProvider::Orbiters), TranslatePick::Into(true), TranslatePick::Agree] {
            assert!(picks.contains(&p), "{p:?}");
        }
        facts.translate.agreed = true;
        assert!(!translate_items(&facts).iter().any(|i| i.action == Action::Set(Pick::Translate(TranslatePick::Agree))), "asked once");
        // Nothing overlaps.
        let items = translate_items(&facts);
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                assert!(!a.rect.intersects(b.rect) || a.rect.intersect(b.rect).area() < 1.0, "{:?} {:?}", a.action, b.action);
            }
        }
    }

    #[test]
    fn the_status_says_what_it_waits_for() {
        let mut f = TranslateFacts { enabled: true, ..Default::default() };
        f.status.state = TranslateState::Downloading { done: 50, total: 200 };
        assert!(status_words(&f).contains("25"));
        f.status.state = TranslateState::NoLocalTranslator;
        assert!(status_words(&f).contains("LM Studio"));
        f.enabled = false;
        assert!(status_words(&f).starts_with("Off"));
    }
}
