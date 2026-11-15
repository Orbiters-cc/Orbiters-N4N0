//! The beta's one notice about its usage statistics (`crate::usage`): a card at the bottom of the window, over nothing that matters,
//! until it is closed. Recording is on from the start (Enzo, 2026-11-15); the card says plainly what is and is not recorded, and where
//! it is turned off. It never stops anything: the page under it works as it does.

use egui::{pos2, vec2, Id, Order, Rect, Sense, Ui};
use orbiters_i18n::{mark, tr};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

/// At most this wide: wide and low, so that it lies under a page's first rows rather than over them.
pub const WIDTH: f32 = 760.0;

/// What the person did with the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Closed: never shown again.
    Understood,
    /// To Settings, where the switch is (closed as well).
    Settings,
}

pub const TITLE: &str = mark("This beta counts which pages and buttons you use");
pub const BODY: &str = mark("Only their names, in what order and when, with this version of the app, to see what to make easier. Never what you type or say, translations, pictures, QR codes or links, worlds, instances, people, files, or your account.");
pub const OFF: &str = mark("You can turn it off at any time in Settings › Beta usage statistics.");

/// Shows the card in `page` (the page's area, right of the sidebar) while `told` is false.
pub fn show(ui: &mut Ui, page: Rect, told: bool) -> Option<Answer> {
    if told {
        return None;
    }
    let ctx = ui.ctx().clone();
    let id = Id::unique("orbiters-usage-notice");
    let enter = anim::spring_from(&ctx, id.with("enter"), [0.0], [1.0], Spring::SOFT)[0];
    let pad = 20.0;
    let width = (page.width() - 48.0).clamp(320.0, WIDTH);
    let text_w = width - pad * 2.0 - 40.0;
    let title = Txt::new(tr(TITLE), 14.0).weight(weight::SEMIBOLD).color(theme::FG).line_height(20.0);
    let body = Txt::new(tr(BODY), 12.5).color(theme::FG_2).line_height(18.0);
    let off = Txt::new(tr(OFF), 12.5).color(theme::FG_3).line_height(18.0);
    let tg = ui.painter().layout_job(title.job(text_w));
    let bg = ui.painter().layout_job(body.job(text_w));
    let og = ui.painter().layout_job(off.job(text_w));
    let buttons_h = 32.0;
    let h = pad + tg.size().y + 6.0 + bg.size().y + 6.0 + og.size().y + 14.0 + buttons_h + pad;
    // At the page's foot from its left, under its first rows in a window of the usual size.
    let base = Rect::from_min_size(pos2(page.left() + 24.0, page.bottom() - 24.0 - h), vec2(width, h));
    let rect = base.translate(vec2(0.0, 30.0 * (1.0 - enter)));
    let mut answer = None;
    egui::Area::new(id).order(Order::Foreground).fixed_pos(rect.min).interactable(true).show(&ctx, |ui| {
        ui.multiply_opacity(enter.clamp(0.0, 1.0));
        // The card itself takes the pointer: nothing under it is pressed through it.
        ui.interact(rect, id.with("card"), Sense::click());
        let p = ui.painter().clone();
        widgets::panel(&p, rect, 18.0);
        let chip = Rect::from_min_size(rect.min + vec2(pad, pad), vec2(28.0, 28.0));
        p.rect_filled(chip, theme::radius(9.0), theme::alpha(theme::ACCENT, 0.16));
        widgets::icon(&p, chip.center(), icons::CHART_LINE, 16.0, theme::ACCENT);
        let x = chip.right() + 12.0;
        let mut y = rect.top() + pad;
        p.galley(pos2(x, y), tg.clone(), theme::FG);
        y += tg.size().y + 6.0;
        p.galley(pos2(x, y), bg.clone(), theme::FG_2);
        y += bg.size().y + 6.0;
        p.galley(pos2(x, y), og.clone(), theme::FG_3);
        y += og.size().y + 14.0;
        let ok = widgets::Button::new(tr("Got it")).variant(widgets::Variant::Primary).size(widgets::Size::Sm);
        let ok_w = ok.desired_width(ui);
        if ok.show_at(ui, Rect::from_min_size(pos2(x, y), vec2(ok_w, buttons_h))).clicked() {
            answer = Some(Answer::Understood);
        }
        let settings = widgets::Button::new(tr("Open Settings")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm);
        let settings_w = settings.desired_width(ui);
        if settings.show_at(ui, Rect::from_min_size(pos2(x + ok_w + 8.0, y), vec2(settings_w, buttons_h))).clicked() {
            answer = Some(Answer::Settings);
        }
    });
    answer
}
