//! N4N0's chat at the top of the window (REQUEST21 3, Enzo: "The N4N0 chat input will be moved at the top center of
//! the tool window … visible whatever the page we are in. When Typing something, it will then transform in a fluid
//! motion … into a chat layout so the discussion can continue").
//!
//! A bar in the middle of the title strip ([`layout`]) takes what is typed to N4N0, on every page. Typing in it (or
//! returning to a conversation, or Ctrl+J / ⌘J with one under way) grows that same shape, in one spring ([`morph`]),
//! into the chat: anchored where the bar is, centred, over the page, which blurs and darkens behind it as it grows
//! ([`orbiters_ui::backdrop`]), more next to the chat than far from it. The text field rides from the bar down to the
//! chat's composer as it grows. Esc, a press beside the chat or its close button shrink it back into the bar the same
//! way (from wherever it is: a reversal half-way is one motion); the conversation stays, and the bar says there is one.
//!
//! The conversation is N4N0's own (the runtime's lines, its questions waiting for a yes, its action log: the same as the
//! Ask page beside VRChat's menu shows), sent with [`N4n0Command::Ask`], answered with [`N4n0Command::AskAnswer`]; what
//! is being typed is kept here, across pages. While the bar or the chat has the keyboard, the page under it gets no keys
//! ([`ChatBar::before_page`]), and while the chat is open no presses or wheel either: one scroll region, the chat's.

pub mod layout;
pub mod morph;
mod panel;
#[cfg(test)]
mod tests;

use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::state::{AccountState, N4n0Who, Snapshot};
use egui::{pos2, vec2, Align2, Area, CursorIcon, Event, Id, Key, KeyboardShortcut, LayerId, Modifiers, Order, Rect, Sense, Ui};
use layout::{Bar, Strip};
use morph::Morph;
use orbiters_i18n::tr;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{anim, icons, widgets};

/// Ctrl+J (⌘J on a Mac): to the bar from anywhere in the window. Nothing else of the window uses it (Ctrl+K is the
/// Settings page's search, REQUEST21 2).
pub const SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::J);

/// The text field's id: one field, in the bar or in the chat's composer.
pub fn field_id() -> Id {
    Id::unique("orbiters-chat-field")
}

/// The layer behind the chat that blurs the page and takes the presses beside the chat.
fn backdrop_layer() -> LayerId {
    LayerId::new(Order::Middle, Id::unique("orbiters-chat-backdrop"))
}

/// The page behind the open chat: blurred (gaussian sigma, points) next to the chat, 40 % of that far from it, and
/// darker.
const BEHIND: orbiters_ui::backdrop::Look = orbiters_ui::backdrop::Look {
    strength: 0.0,
    blur: 16.0,
    dim: 0.30,
    focus: Rect::NOTHING,
    radius: layout::PANEL_RADIUS,
    reach: 420.0,
    far: 0.4,
    base: theme::INK_950,
};

/// Whether N4N0 can answer here, and if not, why (what the bar and the chat say, quietly, and where they lead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Ready,
    /// Not signed in to Orbiters (or its sign-in expired): Sign in.
    SignedOut,
    /// The assistant (or N4N0 altogether) is off: Turn on.
    Off,
}

pub fn gate(snap: &Snapshot) -> Gate {
    if !snap.settings.n4n0.enabled || !snap.settings.n4n0.assistant {
        return Gate::Off;
    }
    match snap.account.state {
        AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. } => Gate::Ready,
        _ => Gate::SignedOut,
    }
}

/// A conversation is under way: something was said, or N4N0 waits for a yes.
pub fn has_conversation(snap: &Snapshot) -> bool {
    !snap.n4n0.lines.is_empty() || !snap.n4n0.confirmations.is_empty() || snap.n4n0.thinking || snap.n4n0.listening
}

/// N4N0's answers so far (an answer that came while the chat was closed lights the bar).
fn answers(snap: &Snapshot) -> usize {
    snap.n4n0.lines.iter().filter(|l| l.who == N4n0Who::N4n0 && !l.streaming).count()
}

/// The chat's state in the window.
#[derive(Default)]
pub struct ChatBar {
    /// What is being typed (kept across pages, and when the chat shrinks back).
    draft: String,
    morph: Morph,
    /// Talk is held (on the bar's or the composer's microphone).
    talking: bool,
    /// An input method is composing (Japanese): Enter is its own until it commits.
    composing: bool,
    /// The chat shows what N4N0 did instead of the conversation.
    log: bool,
    /// Keyboard events of this frame held back from the page (the bar or the chat has the keyboard).
    held: Vec<Event>,
    /// The field is to take the keyboard on the next frame it is drawn.
    focus: bool,
    /// The streamed answer as far as it is shown: its line (by time) and how many characters of it.
    reveal: Option<(u64, f32)>,
    /// N4N0's answers seen with the chat open.
    seen: usize,
    /// A message's text copied, and when (its button shows a check for a moment).
    copied: Option<(u64, f64)>,
    /// The thread stays at its end as it grows (until the person scrolls up).
    follow: bool,
    /// The thread's length last frame (a new message brings the end back into view).
    count: usize,
    /// Where the strip put the bar last frame.
    strip: Option<Strip>,
}

/// Keyboard events: what the page does not get while the chat has the keyboard.
fn is_key_event(e: &Event) -> bool {
    matches!(e, Event::Key { .. } | Event::Text(_) | Event::Paste(_) | Event::Copy | Event::Cut | Event::Ime(_))
}

impl ChatBar {
    /// Whether the chat is open or opening.
    pub fn is_open(&self) -> bool {
        self.morph.is_open()
    }

    /// Whether the chat shows at all (open, or still shrinking back).
    pub fn shows(&self) -> bool {
        !self.morph.closed()
    }

    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Shows what N4N0 did instead of the conversation (the chat's head does it; screenshots too).
    pub fn show_log(&mut self, on: bool) {
        self.log = on;
    }

    /// Where the title strip put the bar in the last frame.
    pub fn strip(&self) -> Option<Strip> {
        self.strip
    }

    /// Opens the chat (from the bar, the shortcut, the button of a narrow strip), the keyboard in its composer.
    pub fn open(&mut self) {
        self.morph.set_open(true);
        self.focus = true;
    }

    /// Shrinks the chat back into the bar; what was typed and the conversation stay.
    pub fn close(&mut self, ctx: &egui::Context) {
        self.morph.set_open(false);
        self.focus = false;
        ctx.memory_mut(|m| m.surrender_focus(field_id()));
    }

    /// Lets go of a held Talk (the window closes).
    pub fn leave(&mut self, ctl: &Ctl) {
        if self.talking {
            self.talking = false;
            ctl.n4n0(N4n0Command::Talk { down: false });
        }
    }

    /// Before the page is drawn: the shortcut, Esc, and the page's keys held back while the chat has the keyboard.
    pub fn before_page(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        let focused = ctx.memory(|m| m.has_focus(field_id()));
        if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT)) {
            self.focus = true;
            // With a conversation to go back to, or something to say about why N4N0 cannot answer: the chat itself.
            if has_conversation(snap) || gate(snap) != Gate::Ready || matches!(self.strip.map(|s| s.bar), Some(Bar::Icon(_))) {
                self.open();
            }
        }
        let has_keys = focused || self.morph.is_open() || self.focus;
        if has_keys && !self.composing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close(ctx);
        }
        if has_keys {
            self.held = ctx.input_mut(|i| {
                let (keys, rest): (Vec<Event>, Vec<Event>) = std::mem::take(&mut i.events).into_iter().partition(is_key_event);
                i.events = rest;
                keys
            });
        }
    }

    /// After the page and the title strip: the bar in the strip (placed by `strip`), the chat when it shows, and the
    /// page behind it blurred. `gpu` is the window's (none: the page is only darkened).
    pub fn show(&mut self, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, strip: Strip, gpu: Option<&crate::gpu::Handles>) {
        let ctx = ui.ctx().clone();
        self.strip = Some(strip);
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            // Typing into the open chat after a press elsewhere in it (a copy, a Yes): the composer takes it.
            if self.morph.is_open() && held.iter().any(|e| matches!(e, Event::Text(_) | Event::Paste(_))) {
                ctx.memory_mut(|m| m.request_focus(field_id()));
            }
            ctx.input_mut(|i| i.events.extend(held));
        }
        self.follow_input_method(&ctx);
        let now = anim::time(&ctx);
        if self.morph.step(now, snap.settings.reduce_motion) {
            ctx.request_repaint();
        }
        if self.morph.is_open() {
            self.seen = answers(snap);
        }
        let panel = layout::panel(strip.window);
        let bar = strip.bar.rect();
        if self.morph.openness() > 0.0 {
            let (focus, radius) = if self.morph.reduced() { (panel, layout::PANEL_RADIUS) } else { layout::shape(bar, panel, self.morph.shape()) };
            self.backdrop(&ctx, strip, focus, radius, gpu);
        }
        if self.morph.reduced() {
            // No morph: the bar stays, the chat fades in over it.
            self.draw(&ctx, ctl, snap, strip, 0.0, 1.0, !self.morph.is_open(), Id::unique("orbiters-chat-bar"));
            // Over the bar (only the chat is raised: egui raises what asks in no set order).
            if self.morph.alpha() > 0.0 {
                self.draw(&ctx, ctl, snap, strip, 1.0, self.morph.alpha(), self.morph.is_open(), Id::unique("orbiters-chat"));
            }
        } else {
            self.draw(&ctx, ctl, snap, strip, self.morph.shape(), 1.0, true, Id::unique("orbiters-chat"));
        }
        if self.focus {
            ctx.memory_mut(|m| m.request_focus(field_id()));
            self.focus = false;
        }
    }

    /// The page behind the chat, blurred and darkened as far as it has opened; a press on it closes the chat.
    fn backdrop(&mut self, ctx: &egui::Context, strip: Strip, focus: Rect, radius: f32, gpu: Option<&crate::gpu::Handles>) {
        let body = Rect::from_min_max(pos2(strip.window.left(), strip.window.top() + strip.title_h), strip.window.max);
        let strength = self.morph.openness();
        let mut close = false;
        Area::new(backdrop_layer().id).order(Order::Middle).fixed_pos(body.min).interactable(true).constrain(false).show(ctx, |ui| {
            let resp = ui.allocate_rect(body, Sense::click());
            let look = orbiters_ui::backdrop::Look { strength, focus, radius, ..BEHIND };
            let blurred = gpu.is_some_and(|gpu| {
                orbiters_ui::backdrop::behind(ui.painter(), Id::unique("orbiters-chat-behind"), &gpu.textures, LayerId::background(), egui::layers::ShapeIdx(0), body, look)
            });
            // Without the window's GPU (a test), or for the one frame a new backdrop meets a changing font atlas, there
            // is nothing to blur with: the page only darkens.
            if !blurred {
                ui.painter().rect_filled(body, 0.0, theme::black(BEHIND.dim * strength));
            }
            close = resp.clicked() && self.morph.is_open();
        });
        if close {
            self.close(ctx);
        }
    }

    /// The bar and the chat as one shape at `t` (0 the bar, 1 the chat), seen at `alpha`; the text field in it when
    /// `field`.
    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, ctx: &egui::Context, ctl: &Ctl, snap: &Snapshot, strip: Strip, t: f32, alpha: f32, field: bool, id: Id) {
        let panel = layout::panel(strip.window);
        let bar = strip.bar.rect();
        let (rect, radius) = layout::shape(bar, panel, t);
        Area::new(id).order(Order::Middle).fixed_pos(rect.min).interactable(true).constrain(false).show(ctx, |ui| {
            ui.set_clip_rect(rect.expand(64.0));
            ui.multiply_opacity(alpha);
            let shape = ui.allocate_rect(rect, Sense::click());
            let open_k = t.clamp(0.0, 1.0);
            paint_surface(ui, rect, radius, open_k, ctx.memory(|m| m.has_focus(field_id())), shape.hovered() && open_k < 0.05);
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            inner.set_clip_rect(rect);
            // What the bar shows fades out as soon as the shape grows; the chat's comes in once it is mostly there.
            let bar_alpha = 1.0 - smoothstep(0.0, 0.3, open_k);
            let chat_alpha = smoothstep(0.35, 0.9, open_k);
            if bar_alpha > 0.0 {
                self.bar_contents(&mut inner, ctl, snap, strip.bar, bar_alpha, &shape);
            }
            // The composer is the bar's well travelling down to the chat's foot (the text in it all the way).
            let composer_end = panel::composer_rect(&inner, rect, &self.draft, snap);
            let k = smoothstep(0.0, 1.0, open_k);
            let composer = Rect::from_min_max(bar.min + (composer_end.min - bar.min) * k, bar.max + (composer_end.max - bar.max) * k);
            if open_k > 0.0 && !matches!(strip.bar, Bar::Icon(_)) || open_k > 0.3 {
                let well = theme::alpha(theme::INK_750, smoothstep(0.0, 0.25, open_k));
                inner.painter().rect_filled(composer, theme::radius(egui::lerp(layout::BAR_RADIUS..=20.0, k)), well);
            }
            if chat_alpha > 0.0 {
                let mut chat = inner.new_child(egui::UiBuilder::new().max_rect(panel));
                chat.set_clip_rect(rect);
                chat.multiply_opacity(chat_alpha);
                panel::show(self, &mut chat, ctl, snap, panel, rect, composer, now_ms());
            }
            if field && !(matches!(strip.bar, Bar::Icon(_)) && open_k < 0.05) {
                let in_bar = bar_field(strip.bar, snap);
                let at = layout::field(in_bar, panel::field_rect(composer_end), open_k);
                self.field(&mut inner, ctl, snap, at, open_k, chat_alpha);
            }
            let pressed = widgets::pressed(&shape);
            if pressed && !self.morph.is_open() {
                // A press on the bar: to the field, or back to the conversation (or to why N4N0 cannot answer).
                self.focus = true;
                if has_conversation(snap) || gate(snap) != Gate::Ready || matches!(strip.bar, Bar::Icon(_)) {
                    self.open();
                }
            }
        });
        // The chat over its backdrop (shown after it, the backdrop would be on top) and over the page's windows. Only
        // this one area asks: egui raises what asks in no set order.
        if id == Id::unique("orbiters-chat") {
            ctx.move_to_top(LayerId::new(Order::Middle, id));
        }
    }

    /// What the bar shows at rest: N4N0's face, what to do (or why not), the shortcut and the microphone.
    fn bar_contents(&mut self, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, bar: Bar, alpha: f32, shape: &egui::Response) {
        let rect = bar.rect();
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha);
        let unread = answers(snap) > self.seen && !self.morph.is_open();
        if let Bar::Icon(_) = bar {
            panel::n4n0_face(&p, rect.center(), 9.0);
            if unread {
                p.circle_filled(rect.center() + vec2(8.0, -7.0), 3.5, theme::ACCENT);
                p.circle_stroke(rect.center() + vec2(8.0, -7.0), 3.5, egui::Stroke::new(1.5, theme::INK_900));
            }
            let _ = shape.clone().on_hover_text(tr!("Talk to N4N0 ({shortcut})", shortcut = shortcut_text())).on_hover_cursor(CursorIcon::PointingHand);
            return;
        }
        panel::n4n0_face(&p, pos2(rect.left() + 15.0, rect.center().y), 8.0);
        if unread {
            p.circle_filled(pos2(rect.left() + 21.0, rect.center().y - 6.5), 3.0, theme::ACCENT);
        }
        let mut right = rect.right() - 6.0;
        if gate(snap) == Gate::Ready && snap.n4n0.voice.installed {
            let mic = Rect::from_center_size(pos2(right - 11.0, rect.center().y), vec2(22.0, 22.0));
            // Held on the bar, N4N0 listens there; let go, the chat opens on what was heard.
            if self.talk_button(ui, ctl, mic, 13.0, alpha, "chat-bar-talk") == Some(false) {
                self.open();
            }
            right = mic.left() - 4.0;
        }
        if rect.width() >= layout::BAR_MIN + 50.0 {
            let key = Txt::new(shortcut_text(), 10.5).weight(weight::MEDIUM).color(theme::FG_4);
            let size = widgets::text_size(ui, &key);
            let cap = Rect::from_min_size(pos2(right - size.x - 12.0, rect.center().y - 9.0), vec2(size.x + 12.0, 18.0));
            p.rect_filled(cap, theme::radius(6.0), theme::white(0.05));
            widgets::text(&p, cap.center(), Align2::CENTER_CENTER, &key);
        }
        // The hint, while nothing is typed (the field draws what is).
        if self.draft.is_empty() {
            let hint = match gate(snap) {
                Gate::SignedOut => tr("Sign in to talk to N4N0"),
                Gate::Off if !snap.settings.n4n0.enabled => tr("N4N0 is off"),
                Gate::Off => tr("N4N0's assistant is off"),
                Gate::Ready if unread => tr("N4N0 answered"),
                Gate::Ready if has_conversation(snap) => tr("Continue with N4N0"),
                Gate::Ready => tr("Talk to N4N0"),
            };
            let field = bar_field(bar, snap);
            let color = if unread { theme::FG_2 } else { theme::FG_3 };
            widgets::text_truncated(&p, pos2(field.left(), rect.center().y), Align2::LEFT_CENTER, &Txt::new(hint, 13.0).color(color), field.width());
        }
        if gate(snap) == Gate::Ready && self.morph.closed() {
            let _ = shape.clone().on_hover_cursor(CursorIcon::Text);
        } else {
            let _ = shape.clone().on_hover_cursor(CursorIcon::PointingHand);
        }
    }

    /// Hold to talk: N4N0 listens while it is held (as the menus' Talk does). Says when it was pressed or let go.
    fn talk_button(&mut self, ui: &mut Ui, ctl: &Ctl, rect: Rect, glyph: f32, alpha: f32, salt: &str) -> Option<bool> {
        let resp = ui.interact(rect, Id::unique(salt), Sense::click_and_drag()).on_hover_text(tr("Hold to talk"));
        let held = resp.is_pointer_button_down_on();
        let ctx = ui.ctx().clone();
        let hover = anim::toggle(&ctx, resp.id.with("hover"), resp.hovered() || held, anim::Spring::SNAPPY);
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha);
        let live = self.talking;
        p.circle_filled(rect.center(), rect.width() / 2.0, if live { theme::alpha(theme::ACCENT, 0.9) } else { theme::white(0.08 * hover) });
        widgets::icon(&p, rect.center(), icons::MICROPHONE, glyph, if live { theme::ON_ACCENT } else { theme::mix(theme::FG_3, theme::FG, hover) });
        if held == self.talking {
            return None;
        }
        self.talking = held;
        ctl.n4n0(N4n0Command::Talk { down: held });
        Some(held)
    }

    /// Tracks an input method's composition: while it composes, Enter and Esc are its own.
    fn follow_input_method(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            for e in &i.events {
                if let Event::Ime(ime) = e {
                    self.composing = match ime {
                        egui::ImeEvent::Preedit { text, .. } => !text.is_empty(),
                        _ => false,
                    };
                }
            }
        });
    }

    /// The one text field: in the bar at `t` 0, in the composer at 1, riding between them as the shape grows.
    fn field(&mut self, ui: &mut Ui, ctl: &Ctl, snap: &Snapshot, at: Rect, t: f32, chat_alpha: f32) {
        let ready = gate(snap) == Gate::Ready;
        let focused = ui.memory(|m| m.has_focus(field_id()));
        // Enter sends (Shift+Enter is a new line); never while an input method composes, nor the Enter that commits it.
        let committed = ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Ime(egui::ImeEvent::Commit(_)))));
        let enter = focused && ready && !self.composing && !committed && take_plain_enter(ui);
        let font = theme::font(egui::lerp(13.0..=14.0, t), weight::REGULAR);
        let rows = if t < 0.5 { 1 } else { panel::draft_rows(ui, &self.draft, at.width()) };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at));
        child.set_clip_rect(at.expand2(vec2(2.0, 2.0)).intersect(ui.clip_rect()));
        let before = self.draft.clone();
        let edit = egui::TextEdit::multiline(&mut self.draft)
            .id(field_id())
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(font)
            .text_color(theme::FG)
            .desired_width(at.width())
            .desired_rows(rows)
            .interactive(ready)
            .return_key(Some(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)));
        let resp = if rows > panel::MAX_ROWS {
            // A very long message scrolls in its own field, the chat's thread being the one place answers scroll.
            egui::ScrollArea::vertical().id_salt("chat-draft").max_height(at.height()).stick_to_bottom(true).show(&mut child, |ui| ui.add(edit)).inner
        } else {
            child.add(edit)
        };
        if t > 0.5 && self.draft.is_empty() {
            let hint = match gate(snap) {
                Gate::Ready => tr("Ask N4N0 anything…"),
                Gate::SignedOut => tr("Sign in to talk to N4N0"),
                Gate::Off => tr("N4N0's assistant is off"),
            };
            let hint = Txt::new(hint, 14.0).color(theme::FG_4);
            let mut p = ui.painter().clone();
            p.multiply_opacity(chat_alpha);
            widgets::text_truncated(&p, pos2(at.left(), at.top() + 9.5), Align2::LEFT_CENTER, &hint, at.width());
        }
        if resp.gained_focus() && !self.morph.is_open() && has_conversation(snap) {
            self.open();
        }
        if self.draft != before && !self.draft.trim().is_empty() && !self.morph.is_open() {
            // Typing grows the bar into the chat.
            self.open();
        }
        if enter {
            self.send(ctl, snap);
        }
    }

    /// Sends what is typed (not while N4N0 still answers the last one, as the runtime would refuse it).
    pub fn send(&mut self, ctl: &Ctl, snap: &Snapshot) -> bool {
        let text = self.draft.trim().to_string();
        if text.is_empty() || snap.n4n0.thinking || gate(snap) != Gate::Ready {
            return false;
        }
        self.draft.clear();
        self.follow = true;
        self.log = false;
        self.open();
        ctl.n4n0(N4n0Command::Ask { text });
        true
    }
}

/// Takes a plain Enter out of this frame's events (egui's `consume_key` would take Shift+Enter for it too).
fn take_plain_enter(ui: &Ui) -> bool {
    ui.input_mut(|i| {
        let at = i.events.iter().position(|e| matches!(e, Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command));
        at.map(|at| i.events.remove(at)).is_some()
    })
}

/// Where the text is in the bar: after N4N0's face, before the shortcut and the microphone.
fn bar_field(bar: Bar, snap: &Snapshot) -> Rect {
    let rect = bar.rect();
    let mut right = rect.right() - 10.0;
    if gate(snap) == Gate::Ready && snap.n4n0.voice.installed {
        right -= 26.0;
    }
    if rect.width() >= layout::BAR_MIN + 50.0 {
        right -= 52.0;
    }
    match bar {
        Bar::Icon(r) => Rect::from_center_size(r.center(), vec2(0.0, 18.0)),
        Bar::Full(_) => Rect::from_min_max(pos2(rect.left() + 30.0, rect.center().y - 9.0), pos2(right.max(rect.left() + 60.0), rect.center().y + 9.0)),
    }
}

/// The bar's surface (a soft well in the strip, ringed while it has the keyboard) growing into the chat's glass (a
/// shade lighter, nearly opaque, with a deep shadow): one fill, its colour and shadow following the shape.
fn paint_surface(ui: &Ui, rect: Rect, radius: f32, k: f32, focused: bool, hovered: bool) {
    let ctx = ui.ctx();
    let p = ui.painter();
    let hover = anim::toggle(ctx, Id::unique("orbiters-chat-hover"), hovered, anim::Spring::SNAPPY);
    let ring = anim::toggle(ctx, Id::unique("orbiters-chat-ring"), focused && k < 0.12, anim::Spring::SNAPPY);
    if k > 0.0 {
        let shadow = egui::epaint::Shadow { offset: [0, (22.0 * k) as i8], blur: (60.0 * k) as u8, spread: 0, color: theme::black(0.55 * k) };
        p.add(shadow.as_shape(rect, theme::radius(radius)));
    }
    let rest = theme::mix(theme::white(0.055), theme::white(0.085), hover.max(ring));
    let open = theme::alpha(theme::INK_850, 0.94);
    p.rect_filled(rect, theme::radius(radius), theme::mix(rest, open, smoothstep(0.0, 0.5, k)));
    // A hairline of light along the glass's top edge.
    if k > 0.0 {
        p.rect_stroke(rect, theme::radius(radius), egui::Stroke::new(1.0, theme::white(0.06 * k)), egui::StrokeKind::Inside);
    }
    if ring > 0.01 {
        p.rect_stroke(rect, theme::radius(radius), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.75 * ring)), egui::StrokeKind::Outside);
    }
}

/// The shortcut as the keyboard shows it.
pub fn shortcut_text() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘J"
    } else {
        "Ctrl J"
    }
}

pub(crate) fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let k = ((x - from) / (to - from)).clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
