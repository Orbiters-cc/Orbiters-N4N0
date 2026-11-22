//! The open chat (REQUEST21 3), after the website's AI playground (`/admin?tab=ai&aiTab=playground`): one readable
//! column; the person's messages in a soft bubble on the right, N4N0's answers as plain text under its name, revealed
//! smoothly as they stream with a pulsing dot at their end; what N4N0 did as quiet steps; a question waiting for a yes
//! as a card with Yes and No; failures in a tinted box; a copy button under each message; the composer docked at the
//! foot (Enter sends, Shift+Enter starts a line, a round send button, the microphone to hold). Filled surfaces, no
//! outlines. The thread is the chat's one scroll region, and it keeps its end in view as answers grow.

use super::{gate, has_conversation, ChatBar, Gate};
use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::overlay::orbit::panels::{TalkLogRow, TalkTone};
use crate::state::{AccountState, N4n0Confirmation, N4n0Line, N4n0Status, N4n0Who, Snapshot};
use egui::{pos2, vec2, Align2, CornerRadius, Id, Painter, Pos2, Rect, Sense, Ui};
use orbiters_i18n::{mark, tr};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{anim, icons};

/// The composer grows with what is typed up to this many lines; past them its text scrolls in it.
pub const MAX_ROWS: usize = 6;
/// The thread's column, at most (the playground's 780, for a narrower panel).
const COLUMN: f32 = 640.0;
/// A line of the composer, and its padding above and below.
const ROW: f32 = 20.0;
const COMPOSER_PAD: f32 = 12.0;
/// The line under the composer (how to send).
const HINT_H: f32 = 22.0;
/// The head of the chat.
const HEAD_H: f32 = 56.0;
/// Margins of the composer to the chat's sides and foot.
const DOCK: f32 = 16.0;

/// Lines the draft takes at `width` (1 to well past [`MAX_ROWS`]).
pub fn draft_rows(ui: &Ui, draft: &str, width: f32) -> usize {
    if draft.is_empty() {
        return 1;
    }
    let galley = ui.painter().layout_job(Txt::new(draft, 14.0).job(width.max(40.0)));
    galley.rows.len().max(1)
}

/// The composer at the foot of the shape `shape` (it rides down with the shape's foot as it grows).
pub fn composer_rect(ui: &Ui, shape: Rect, draft: &str, _snap: &Snapshot) -> Rect {
    let width = shape.width() - 2.0 * DOCK;
    let rows = draft_rows(ui, draft, field_width(width)).min(MAX_ROWS) as f32;
    let h = 2.0 * COMPOSER_PAD + rows * ROW + 4.0;
    Rect::from_min_size(pos2(shape.left() + DOCK, shape.bottom() - DOCK - HINT_H - h), vec2(width, h))
}

fn field_width(composer_width: f32) -> f32 {
    composer_width - 48.0 - 52.0
}

/// The text field in the composer: between the microphone and the send button.
pub fn field_rect(composer: Rect) -> Rect {
    Rect::from_min_max(pos2(composer.left() + 48.0, composer.top() + COMPOSER_PAD + 2.0), pos2(composer.left() + 48.0 + field_width(composer.width()), composer.bottom() - COMPOSER_PAD))
}

/// One step of the thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Item<'a> {
    You(&'a N4n0Line),
    /// An answer; `first` of N4N0's in a row (its name shows over it).
    N4n0 { line: &'a N4n0Line, first: bool },
    /// Something N4N0 did or noticed.
    Note(&'a str),
    Error(&'a str),
    /// Before the first words of an answer.
    Thinking,
    /// N4N0 hears the person (the microphone's level).
    Listening(f32),
    /// What N4N0 does right now ("Doing: …").
    Doing(&'a str),
    /// A question waiting for the person's yes (here, or on Orbiters).
    Confirm(&'a N4n0Confirmation),
}

/// The thread, in order: the conversation, what is under way, the questions waiting.
pub fn items(n4n0: &N4n0Status) -> Vec<Item<'_>> {
    // The question N4N0 asks is said by its card: its note line would only repeat it.
    let asked: Vec<&str> = n4n0.confirmations.iter().map(|c| c.summary.as_str()).collect();
    let mut out = Vec::new();
    for line in &n4n0.lines {
        match line.who {
            N4n0Who::You => out.push(Item::You(line)),
            N4n0Who::N4n0 => {
                let first = !matches!(out.last(), Some(Item::N4n0 { .. }));
                out.push(Item::N4n0 { line, first });
            }
            N4n0Who::Note if asked.iter().any(|q| !q.is_empty() && (*q == line.text || q.starts_with(line.text.as_str()))) => {}
            N4n0Who::Note => out.push(Item::Note(&line.text)),
            N4n0Who::Error => out.push(Item::Error(&line.text)),
        }
    }
    let arriving = n4n0.lines.last().is_some_and(|l| l.streaming);
    if n4n0.listening {
        out.push(Item::Listening(n4n0.voice.level));
    } else if n4n0.thinking && !arriving {
        out.push(Item::Thinking);
    }
    if let Some(doing) = n4n0.assistant.doing.as_deref() {
        out.push(Item::Doing(doing));
    }
    out.extend(n4n0.confirmations.iter().map(Item::Confirm));
    out
}

/// How the assistant stands, under N4N0's name in the chat's head.
pub fn status_line(snap: &Snapshot) -> String {
    let (n4n0, settings) = (&snap.n4n0, &snap.settings);
    if !settings.n4n0.enabled {
        return tr("N4N0 is off").into();
    }
    if !settings.n4n0.assistant {
        return tr("Assistant off: N4N0 is a tool without AI").into();
    }
    match &snap.account.state {
        AccountState::Offline { .. } => return tr("Offline: N4N0 does the short commands it knows").into(),
        AccountState::SignedIn { .. } | AccountState::Checking { .. } => {}
        _ => return tr("Sign in to Orbiters (Settings) to talk to N4N0").into(),
    }
    match &n4n0.quota {
        Some(q) if q.own_key => tr("Answers on your own key").into(),
        Some(q) if q.limit > 0 => tr!("{left} of {limit} free answers left today", left = q.limit.saturating_sub(q.used), limit = q.limit),
        _ => tr("Ask anything, or hold Talk and speak").into(),
    }
}

/// The chat in `panel` (laid out at its full size, seen through the growing `shape`), its composer at `composer`.
#[allow(clippy::too_many_arguments)]
pub fn show(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, panel: Rect, shape: Rect, composer: Rect, now_ms: u64) {
    head(bar, ui, ctl, snap, panel);
    let thread = Rect::from_min_max(pos2(panel.left(), panel.top() + HEAD_H), pos2(panel.right(), composer.top() - 6.0));
    let mut tu = ui.new_child(egui::UiBuilder::new().max_rect(thread));
    tu.set_clip_rect(thread.intersect(shape));
    match gate(snap) {
        Gate::Ready if bar.log => log(&mut tu, snap, now_ms),
        Gate::Ready => conversation(bar, &mut tu, ctl, snap, thread),
        g => gated(&mut tu, ctl, snap, thread, g),
    }
    dock(bar, ui, ctl, snap, composer);
}

/// N4N0's name and how it stands; the action log, a new conversation, and close.
fn head(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, panel: Rect) {
    let p = ui.painter().clone();
    let y = panel.top() + HEAD_H / 2.0;
    n4n0_face(&p, pos2(panel.left() + 30.0, y), 12.0);
    widgets::text(&p, pos2(panel.left() + 50.0, y - 8.0), Align2::LEFT_CENTER, &Txt::new("N4N0", 14.0).weight(weight::SEMIBOLD));
    let mut right = panel.right() - 14.0;
    let close = Rect::from_min_size(pos2(right - 32.0, y - 16.0), vec2(32.0, 32.0));
    if IconButton::new(icons::CARET_UP, mark("Close (Esc)")).size(32.0, 15.0).id_salt("chat-close").show_at(ui, close).clicked() {
        bar.close(ui.ctx());
    }
    right = close.left() - 4.0;
    if has_conversation(snap) {
        let new = Rect::from_min_size(pos2(right - 32.0, y - 16.0), vec2(32.0, 32.0));
        if IconButton::new(icons::PENCIL_SIMPLE, mark("New conversation")).size(32.0, 15.0).id_salt("chat-new").show_at(ui, new).clicked() {
            ctl.n4n0(N4n0Command::NewConversation);
            bar.log = false;
            bar.reveal = None;
            bar.focus = true;
        }
        right = new.left() - 6.0;
    }
    let log = &snap.n4n0.assistant.log;
    if !log.is_empty() || bar.log {
        let label = if bar.log { tr("Back to the conversation").to_string() } else { format!("{} · {}", tr("What N4N0 did"), log.len()) };
        let b = Button::new(&label).icon(if bar.log { icons::ARROW_LEFT } else { icons::LIST_CHECKS }).variant(Variant::Ghost).size(Size::Sm).id_salt("chat-log");
        let w = b.desired_width(ui);
        if b.show_at(ui, Rect::from_min_size(pos2(right - w, y - 14.0), vec2(w, 28.0))).clicked() {
            bar.log = !bar.log;
            bar.focus = true;
        }
        right -= w + 8.0;
    }
    widgets::text_truncated(&p, pos2(panel.left() + 50.0, y + 9.0), Align2::LEFT_CENTER, &Txt::new(status_line(snap), 11.5).color(theme::FG_3), (right - panel.left() - 58.0).max(40.0));
}

/// Why N4N0 cannot answer here, and the one thing that changes it.
fn gated(ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, thread: Rect, g: Gate) {
    let (glyph, title, body, action) = match g {
        Gate::SignedOut => (
            icons::SIGN_IN,
            tr("Sign in to Orbiters to talk to N4N0."),
            tr("N4N0 answers with Orbiters' assistant: sign in with your Orbiters account, in your browser."),
            tr("Sign in"),
        ),
        _ if !snap.settings.n4n0.enabled => (icons::POWER, tr("N4N0 is off"), tr("Turn N4N0 on to talk to it here."), tr("Turn on N4N0")),
        _ => (icons::POWER, tr("N4N0's assistant is off"), tr("Turn it on to talk to N4N0 here. Without it, N4N0 stays a tool without AI."), tr("Turn on the assistant")),
    };
    let c = thread.center() - vec2(0.0, 24.0);
    let p = ui.painter().clone();
    p.circle_filled(c - vec2(0.0, 52.0), 26.0, theme::alpha(theme::ACCENT, 0.12));
    widgets::icon(&p, c - vec2(0.0, 52.0), glyph, 22.0, theme::ACCENT);
    widgets::text(&p, c - vec2(0.0, 10.0), Align2::CENTER_CENTER, &Txt::new(title, 16.0).weight(weight::SEMIBOLD));
    let g2 = p.layout_job(Txt::new(body, 13.0).color(theme::FG_3).line_height(19.0).job(380.0f32.min(thread.width() - 64.0)));
    let body_rect = Rect::from_center_size(c + vec2(0.0, 12.0 + g2.size().y / 2.0), g2.size());
    p.galley(pos2(c.x - g2.size().x / 2.0, body_rect.top()), g2.clone(), theme::FG_3);
    let b = Button::new(action).icon(glyph).variant(Variant::Primary).id_salt("chat-gate");
    let w = b.desired_width(ui);
    if b.show_at(ui, Rect::from_center_size(pos2(c.x, body_rect.bottom() + 32.0), vec2(w, 40.0))).clicked() {
        match g {
            Gate::SignedOut => ctl.sign_in(),
            _ => {
                let mut on = snap.settings.clone();
                on.n4n0.enabled = true;
                on.n4n0.assistant = true;
                ctl.save_changes(&snap.settings, &on);
            }
        }
    }
}

/// The conversation: the column of messages, in the chat's one scroll region.
fn conversation(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, thread: Rect) {
    let all = items(&snap.n4n0);
    if all.is_empty() {
        empty(ui, thread);
        return;
    }
    let grew = all.len() != bar.count;
    bar.count = all.len();
    let column_w = (thread.width() - 56.0).min(COLUMN);
    let now = anim::time(ui.ctx());
    let mut area = egui::ScrollArea::vertical().id_salt("chat-thread").auto_shrink([false, false]).stick_to_bottom(true);
    if grew && bar.follow {
        area = area.vertical_scroll_offset(f32::MAX);
    }
    let out = area.show(ui, |ui| {
        ui.add_space(8.0);
        let last = all.len() - 1;
        for (i, item) in all.iter().enumerate() {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 0.0), Sense::hover());
            let left = row.center().x - column_w / 2.0;
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(left, row.top()), vec2(column_w, f32::INFINITY))));
            item_ui(bar, &mut cu, ctl, item, i == last, now);
            let used = cu.min_rect().height();
            ui.allocate_exact_size(vec2(ui.available_width(), used + 14.0), Sense::hover());
        }
        ui.add_space(6.0);
    });
    // Scrolled up by hand: the end is no longer followed until it is back in view.
    let at_end = out.state.clamped_offset().y + out.inner_rect.height() >= out.content_size.y - 4.0;
    if ui.input(|i| i.smooth_scroll_delta.y > 0.0) && ui.rect_contains_pointer(thread) {
        bar.follow = false;
    } else if at_end {
        bar.follow = true;
    }
}

/// Nothing said yet.
fn empty(ui: &mut Ui, thread: Rect) {
    let p = ui.painter().clone();
    let c = thread.center() - vec2(0.0, 20.0);
    n4n0_face(&p, c - vec2(0.0, 40.0), 24.0);
    widgets::text(&p, c + vec2(0.0, 6.0), Align2::CENTER_CENTER, &Txt::new(tr("Talk to N4N0"), 16.0).weight(weight::SEMIBOLD));
    let g = p.layout_job(
        Txt::new(tr("“Nano, put my hoodie on.” “Nano, plan a movie night Saturday.” “Nano, save this video.”"), 12.5)
            .color(theme::FG_3)
            .line_height(19.0)
            .job(420.0f32.min(thread.width() - 64.0)),
    );
    let mut job_w = 0.0f32;
    for r in &g.rows {
        job_w = job_w.max(r.size.x);
    }
    p.galley(pos2(c.x - job_w / 2.0, c.y + 24.0), g, theme::FG_3);
}

/// One step of the thread in the column `ui`.
#[allow(clippy::too_many_arguments)]
fn item_ui(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, item: &Item<'_>, last: bool, now: f64) {
    let width = ui.available_width();
    match item {
        Item::You(line) => {
            let max = (width * 0.85).min(520.0);
            let g = ui.painter().layout_job(Txt::new(&line.text, 14.0).color(theme::FG).line_height(20.0).job(max - 28.0));
            let size = g.size() + vec2(28.0, 20.0);
            let (row, resp) = ui.allocate_exact_size(vec2(width, size.y), Sense::hover());
            let bubble = Rect::from_min_size(pos2(row.right() - size.x, row.top()), size);
            ui.painter().rect_filled(bubble, CornerRadius { nw: 18, ne: 18, sw: 18, se: 6 }, theme::white(0.075));
            ui.painter().galley(bubble.min + vec2(14.0, 10.0), g, theme::FG);
            copy_row(bar, ui, &line.text, line.at_ms, true, last || resp.hovered(), now);
        }
        Item::N4n0 { line, first } => {
            if *first {
                let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
                n4n0_face(ui.painter(), pos2(r.left() + 9.0, r.center().y), 9.0);
                widgets::text(ui.painter(), pos2(r.left() + 25.0, r.center().y), Align2::LEFT_CENTER, &Txt::new("N4N0", 12.5).weight(weight::SEMIBOLD).color(theme::FG_2));
            }
            let shown = bar.revealed(ui.ctx(), line);
            let g = ui.painter().layout_job(Txt::new(&shown, 14.0).color(theme::FG).line_height(21.0).job(width));
            let (r, resp) = ui.allocate_exact_size(vec2(width, g.size().y.max(21.0)), Sense::hover());
            let end = r.min + g.rows.last().map(|row| vec2(row.rect().right(), row.rect().center().y)).unwrap_or(vec2(0.0, 10.5));
            ui.painter().galley(r.min, g, theme::FG);
            if line.streaming || shown.len() < line.text.len() {
                // The website's streaming mark: a pulsing dot where the text ends.
                let k = (anim::time(ui.ctx()) * 6.0).sin() as f32 * 0.5 + 0.5;
                ui.painter().circle_filled(end + vec2(8.0, 0.0), 3.0 + 0.8 * k, theme::alpha(theme::ACCENT, 0.55 + 0.45 * k));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            } else {
                copy_row(bar, ui, &line.text, line.at_ms, false, last || resp.hovered(), now);
            }
        }
        Item::Note(text) => {
            let g = ui.painter().layout_job(Txt::new(*text, 12.5).color(theme::FG_3).line_height(18.0).job(width - 26.0));
            let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y.max(18.0)), Sense::hover());
            widgets::icon(ui.painter(), pos2(r.left() + 8.0, r.top() + 9.0), icons::CHECK_CIRCLE, 13.0, theme::alpha(theme::ACCENT, 0.8));
            ui.painter().galley(pos2(r.left() + 24.0, r.top()), g, theme::FG_3);
        }
        Item::Error(text) => {
            let g = ui.painter().layout_job(Txt::new(*text, 13.0).color(theme::DANGER).line_height(19.0).job(width - 52.0));
            let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y + 20.0), Sense::hover());
            ui.painter().rect_filled(r, theme::radius(14.0), theme::alpha(theme::DANGER, 0.1));
            widgets::icon(ui.painter(), pos2(r.left() + 20.0, r.top() + 19.5), icons::WARNING_CIRCLE, 15.0, theme::DANGER);
            ui.painter().galley(pos2(r.left() + 38.0, r.top() + 10.0), g, theme::DANGER);
        }
        Item::Thinking | Item::Listening(_) => {
            let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
            n4n0_face(ui.painter(), pos2(r.left() + 9.0, r.center().y), 9.0);
            let t = anim::time(ui.ctx());
            if let Item::Listening(level) = item {
                let track = Rect::from_min_size(pos2(r.left() + 28.0, r.center().y - 3.0), vec2(120.0, 6.0));
                ui.painter().rect_filled(track, theme::radius(3.0), theme::white(0.08));
                ui.painter().rect_filled(Rect::from_min_size(track.min, vec2((track.width() * level.clamp(0.0, 1.0)).max(6.0), 6.0)), theme::radius(3.0), theme::ACCENT);
                widgets::text(ui.painter(), pos2(track.right() + 12.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Listening…"), 12.5).color(theme::FG_3));
            } else {
                for i in 0..3 {
                    let phase = ((t * 2.2 - i as f64 * 0.22).sin() * 0.5 + 0.5) as f32;
                    ui.painter().circle_filled(pos2(r.left() + 30.0 + i as f32 * 11.0, r.center().y - 2.0 * phase), 3.0, theme::alpha(theme::ACCENT, 0.45 + 0.55 * phase));
                }
                widgets::text(ui.painter(), pos2(r.left() + 66.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(tr("N4N0 is thinking…"), 12.5).color(theme::FG_3));
            }
            // Dots and a level meter: 30 frames a second, not the monitor's rate.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        Item::Doing(what) => {
            let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
            let t = anim::time(ui.ctx()) as f32;
            let c = pos2(r.left() + 9.0, r.center().y);
            orbiters_ui::paint::ring(ui.painter(), c, 6.0, 2.0, theme::alpha(theme::ACCENT, 0.25));
            orbiters_ui::paint::arc(ui.painter(), c, 6.0, t * 5.0, 1.8, 2.0, theme::ACCENT);
            widgets::text_truncated(ui.painter(), pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(tr!("Doing: {what}", what = what), 12.5).color(theme::FG_2), width - 28.0);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        Item::Confirm(c) => confirm(bar, ui, ctl, c, width),
    }
}

/// A question N4N0 asks before it does something: answered here with Yes or No (or by saying it), or on Orbiters.
fn confirm(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, c: &N4n0Confirmation, width: f32) {
    let local = c.is_local();
    let title = if c.title.is_empty() { tr("Needs your approval").to_string() } else { c.title.clone() };
    let fallback = if local { tr("N4N0 asks before it does this. Say or type no to leave it.") } else { tr("Waiting for your approval on Orbiters.") };
    let summary = if c.summary.is_empty() { fallback } else { c.summary.as_str() };
    let g = ui.painter().layout_job(Txt::new(summary, 13.0).color(theme::FG_2).line_height(19.0).job(width - 40.0));
    let h = 16.0 + 22.0 + g.size().y + 14.0 + 34.0 + 16.0;
    let (card, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(card, theme::radius(18.0), theme::alpha(theme::ACCENT, 0.08));
    widgets::icon(&p, pos2(card.left() + 28.0, card.top() + 27.0), icons::SHIELD_CHECK, 15.0, theme::ACCENT);
    widgets::text(&p, pos2(card.left() + 44.0, card.top() + 27.0), Align2::LEFT_CENTER, &Txt::new(title, 13.5).weight(weight::SEMIBOLD));
    p.galley(pos2(card.left() + 20.0, card.top() + 16.0 + 26.0), g, theme::FG_2);
    let foot = card.bottom() - 16.0 - 34.0;
    let id = Id::unique(("chat-confirm", &c.id));
    if local {
        let yes = Button::new(tr("Yes, do it")).icon(icons::CHECK).variant(Variant::Primary).size(Size::Sm).id_salt(id.with("yes"));
        let yw = yes.desired_width(ui);
        let yes_rect = Rect::from_min_size(pos2(card.right() - 16.0 - yw, foot), vec2(yw, 34.0));
        let no = Button::new(tr("No, leave it")).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).id_salt(id.with("no"));
        let nw = no.desired_width(ui);
        if no.show_at(ui, Rect::from_min_size(pos2(yes_rect.left() - 8.0 - nw, foot), vec2(nw, 34.0))).clicked() {
            ctl.n4n0(N4n0Command::AskAnswer { yes: false });
            bar.focus = true;
        }
        if yes.show_at(ui, yes_rect).clicked() {
            ctl.n4n0(N4n0Command::AskAnswer { yes: true });
            bar.focus = true;
        }
    } else {
        let go = Button::new(tr("Approve on Orbiters")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(Size::Sm).id_salt(id.with("approve"));
        let w = go.desired_width(ui);
        if go.show_at(ui, Rect::from_min_size(pos2(card.right() - 16.0 - w, foot), vec2(w, 34.0))).clicked() {
            ctl.n4n0(N4n0Command::OpenLink { url: c.approve_url.clone() });
        }
    }
}

/// The row under a message: its copy button (shown on the last message and the one pointed at).
fn copy_row(bar: &mut ChatBar, ui: &mut Ui, text: &str, at_ms: u64, mine: bool, shown: bool, now: f64) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let r = Rect::from_center_size(pos2(if mine { row.right() - 13.0 } else { row.left() + 13.0 }, row.center().y + 2.0), vec2(26.0, 26.0));
    let hovered = ui.rect_contains_pointer(row) || ui.rect_contains_pointer(r);
    let k = anim::toggle(ui.ctx(), Id::unique(("chat-copy-shown", at_ms, mine)), shown || hovered, anim::Spring::SNAPPY);
    if k < 0.01 {
        return;
    }
    let done = bar.copied.is_some_and(|(at, when)| at == at_ms && now - when < 1.6);
    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(r));
    cu.multiply_opacity(k);
    let glyph = if done { icons::CHECK } else { icons::COPY };
    if IconButton::new(glyph, mark("Copy")).size(26.0, 13.0).id_salt(("chat-copy", at_ms, mine)).show_at(&mut cu, r).clicked() {
        ui.ctx().copy_text(text.to_string());
        bar.copied = Some((at_ms, now));
        bar.focus = true;
    }
    if done {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
    }
}

/// What N4N0 did (the assistant's action log, as the Ask page lists it), the newest first.
fn log(ui: &mut Ui, snap: &Snapshot, now_ms: u64) {
    let rows: Vec<TalkLogRow> = crate::overlay::orbit::talk_log_rows_for_shots(&snap.n4n0.assistant.log, now_ms);
    let column = (ui.max_rect().width() - 56.0).min(COLUMN);
    egui::ScrollArea::vertical().id_salt("chat-log").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(8.0);
        if rows.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
            widgets::text(ui.painter(), r.center(), Align2::CENTER_CENTER, &Txt::new(tr("Nothing yet: what N4N0 does for you is listed here."), 12.5).color(theme::FG_3));
        }
        for row in &rows {
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 0.0), Sense::hover());
            let left = line.center().x - column / 2.0;
            let (tone_fg, tone_bg) = match row.tone {
                TalkTone::Done => (theme::ACCENT, theme::alpha(theme::ACCENT, 0.12)),
                TalkTone::Waiting => (theme::WARN, theme::alpha(theme::WARN, 0.12)),
                TalkTone::Wrong => (theme::DANGER, theme::alpha(theme::DANGER, 0.12)),
                TalkTone::Declined => (theme::FG_3, theme::white(0.06)),
            };
            let chip = Txt::new(&row.outcome, 11.0).weight(weight::SEMIBOLD).color(tone_fg);
            let chip_w = widgets::text_size(ui, &chip).x + 18.0;
            let said = ui.painter().layout_job(Txt::new(&row.said, 13.5).color(theme::FG).line_height(19.0).job(column - chip_w - 40.0));
            let detail = ui.painter().layout_job(Txt::new(&row.detail, 11.5).color(theme::FG_3).line_height(16.0).job(column - chip_w - 40.0));
            let h = said.size().y + detail.size().y + 24.0;
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h + 8.0), Sense::hover());
            let card = Rect::from_min_size(pos2(left, r.top()), vec2(column, h));
            ui.painter().rect_filled(card, theme::radius(14.0), theme::white(0.04));
            ui.painter().galley(pos2(card.left() + 16.0, card.top() + 12.0), said.clone(), theme::FG);
            ui.painter().galley(pos2(card.left() + 16.0, card.top() + 12.0 + said.size().y + 2.0), detail, theme::FG_3);
            let pill = Rect::from_min_size(pos2(card.right() - 14.0 - chip_w, card.top() + 12.0), vec2(chip_w, 22.0));
            ui.painter().rect_filled(pill, theme::radius(11.0), tone_bg);
            widgets::text(ui.painter(), pill.center(), Align2::CENTER_CENTER, &chip);
        }
        ui.add_space(6.0);
    });
}

/// The composer's surface, its microphone and send button, and how to send; the field itself is the bar's
/// ([`super::ChatBar::field`]), brought down here.
fn dock(bar: &mut ChatBar, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, composer: Rect) {
    let p = ui.painter().clone();
    p.rect_filled(composer, theme::radius(20.0), theme::INK_750);
    let ready = gate(snap) == Gate::Ready;
    let y = composer.bottom() - 24.0;
    let mic = Rect::from_center_size(pos2(composer.left() + 26.0, y), vec2(32.0, 32.0));
    if ready && snap.n4n0.voice.installed {
        let _ = bar.talk_button(ui, ctl, mic, 16.0, 1.0, "chat-talk");
    } else {
        // No ears yet (or no assistant): the microphone shows where it will be, and says why it waits.
        let r = ui.interact(mic, Id::unique("chat-talk-off"), Sense::hover());
        widgets::icon(&p, mic.center(), icons::MICROPHONE, 16.0, theme::FG_4);
        if ready {
            let _ = r.on_hover_text(tr("Install N4N0's ears to talk instead of typing."));
        }
    }
    let send = Rect::from_center_size(pos2(composer.right() - 26.0, y), vec2(32.0, 32.0));
    let can = ready && !snap.n4n0.thinking && !bar.draft.trim().is_empty();
    let resp = ui.interact(send, Id::unique("chat-send"), if can { Sense::click() } else { Sense::hover() }).on_hover_text(tr("Send (Enter)"));
    widgets::note_control(ui.ctx(), mark("Send (Enter)"), send, can);
    let hover = anim::toggle(ui.ctx(), Id::unique("chat-send-hover"), can && resp.hovered(), anim::Spring::SNAPPY);
    let on = anim::toggle(ui.ctx(), Id::unique("chat-send-on"), can, anim::Spring::SNAPPY);
    let fill = theme::mix(theme::white(0.08), theme::mix(theme::ACCENT, theme::ACCENT_HOVER, hover), on);
    p.circle_filled(send.center(), 16.0, fill);
    widgets::icon(&p, send.center(), icons::ARROW_UP, 16.0, theme::mix(theme::FG_4, theme::ON_ACCENT, on));
    if can && resp.clicked() {
        bar.send(ctl, snap);
    }
    let hint = if !ready {
        String::new()
    } else if snap.n4n0.thinking {
        tr("N4N0 is answering…").to_string()
    } else {
        tr("Enter to send · Shift+Enter for a new line").to_string()
    };
    widgets::text(&p, pos2(composer.center().x, composer.bottom() + HINT_H / 2.0 + 1.0), Align2::CENTER_CENTER, &Txt::new(hint, 11.0).color(theme::FG_4));
}

/// N4N0's little face: a green disc with two eyes.
pub fn n4n0_face(p: &Painter, center: Pos2, radius: f32) {
    p.circle_filled(center, radius, theme::ACCENT);
    let k = radius / 11.0;
    for dx in [-3.6f32, 3.6] {
        p.rect_filled(Rect::from_center_size(center + vec2(dx * k, -0.5 * k), vec2(2.4 * k, 5.0 * k)), theme::radius(1.2 * k), theme::ON_ACCENT);
    }
}

impl ChatBar {
    /// A streamed answer as far as it is shown: it flows in at a speed that grows with what is still to show (the
    /// website's `useSmoothText`), catching up within a few hundred milliseconds; an answer complete when it first
    /// shows is shown whole.
    fn revealed(&mut self, ctx: &egui::Context, line: &N4n0Line) -> String {
        let total = line.text.chars().count() as f32;
        let shown = match self.reveal {
            Some((at, n)) if at == line.at_ms => n,
            _ if line.streaming => 0.0,
            _ => return line.text.clone(),
        };
        let left = total - shown;
        let next = if left > 0.0 {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.064);
            ctx.request_repaint();
            (shown + (left * 5.0).max(60.0) * dt).min(total)
        } else {
            total
        };
        self.reveal = Some((line.at_ms, if line.streaming || next < total { next } else { total }));
        line.text.chars().take(next.floor() as usize).collect()
    }
}
