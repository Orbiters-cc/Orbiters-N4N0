//! Widgets in the Orbiters style. Feedback is immediate on pointer down (press scale, highlight), springs carry
//! every change, and selection-like controls (segments, toggles, navigation) act on press like the web version.

use crate::anim::{self, Spring};
use crate::paint;
use crate::theme::{self, weight, Txt};
use crate::{fonts, icons};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, IdSalt, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by an automated run (the end-to-end test) that clicks controls like a person, by their labels.
static WATCH: AtomicBool = AtomicBool::new(false);

/// Told the English name of every control pressed (the beta's usage statistics, `orbiters-desktop`'s `usage`). Set once by the app.
static PRESS_HOOK: std::sync::OnceLock<fn(&'static str)> = std::sync::OnceLock::new();

thread_local! {
    /// The label of the settings row ([`field`]) whose control is being drawn: what a toggle or slider there is named by.
    static FIELD: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    /// The rows [`field`] drew while [`record_fields`] runs.
    static ROWS: std::cell::RefCell<Option<Vec<FieldRow>>> = const { std::cell::RefCell::new(None) };
    /// The controls drawn while [`record_controls`] runs.
    static CONTROLS: std::cell::RefCell<Option<Vec<ControlMark>>> = const { std::cell::RefCell::new(None) };
}

/// A control drawn while [`record_controls`] ran: what names it (its id salt, or a button's English label), what kind of
/// control it is, and where. Settings claims each one for a setting of its registry, or names it an action or a status.
#[derive(Clone, Debug)]
pub struct ControlMark {
    pub key: String,
    pub kind: &'static str,
    pub rect: Rect,
}

/// Runs `f`, and gives back every control drawn in it: toggles, sliders, segments, text fields, buttons, icon buttons,
/// step sliders, dropdowns, and what a page marks itself ([`mark_control`]).
pub fn record_controls<R>(f: impl FnOnce() -> R) -> (R, Vec<ControlMark>) {
    let was = CONTROLS.with(|c| c.replace(Some(Vec::new())));
    let out = f();
    let marks = CONTROLS.with(|c| c.replace(was)).unwrap_or_default();
    (out, marks)
}

/// Notes a control drawn at `rect` under `key`, while [`record_controls`] runs (a page's own control: a chip, a tile).
pub fn mark_control(key: &str, kind: &'static str, rect: Rect) {
    CONTROLS.with(|c| {
        if let Some(marks) = c.borrow_mut().as_mut() {
            marks.push(ControlMark { key: key.to_owned(), kind, rect });
        }
    });
}

/// Gives the control marked last its rect (marked before its salt was used up).
fn set_last_mark(rect: Rect) {
    CONTROLS.with(|c| {
        if let Some(last) = c.borrow_mut().as_mut().and_then(|m| m.last_mut()) {
            if last.rect == Rect::NOTHING {
                last.rect = rect;
            }
        }
    });
}

/// An id salt as a control's key: a text salt without its quotes.
pub fn salt_key(salt: &impl std::fmt::Debug) -> String {
    format!("{salt:?}").trim_matches('"').to_owned()
}

/// A settings row [`field`] drew while [`record_fields`] ran: its label as shown, where it is, and a shape slot under its
/// text, to paint something behind the row (a search result's highlight) once the page knows which row it wants.
#[derive(Clone)]
pub struct FieldRow {
    pub label: String,
    pub rect: Rect,
    /// The room the row gave its control, on its right, and the width left to its label and description.
    pub control: Rect,
    pub text_width: f32,
    pub behind: (Painter, egui::layers::ShapeIdx),
}

/// Runs `f`, and gives back every row [`field`] drew in it (Settings' search finds and lights its rows by them; its test
/// sees that none is drawn without a registry entry). A nested call keeps its own rows.
pub fn record_fields<R>(f: impl FnOnce() -> R) -> (R, Vec<FieldRow>) {
    let was = ROWS.with(|r| r.replace(Some(Vec::new())));
    let out = f();
    let rows = ROWS.with(|r| r.replace(was)).unwrap_or_default();
    (out, rows)
}

/// Runs `f` without its rows being recorded ([`record_fields`]): rows named by what they show (a parameter, a device found),
/// not settings of their own.
pub fn unrecorded<R>(f: impl FnOnce() -> R) -> R {
    let was = ROWS.with(|r| r.take());
    let out = f();
    ROWS.with(|r| *r.borrow_mut() = was);
    out
}

/// From now on, `hook` hears the English name of every control pressed: buttons, icon buttons, segments, the switches and sliders of a settings
/// row (by the row's label). Only the app's own words are told ([`orbiters_i18n::control_name`]): a label that is a name, a number or a text
/// with values in it is not. Nothing is told while no hook is set.
pub fn on_press(hook: fn(&'static str)) {
    let _ = PRESS_HOOK.set(hook);
}

/// A control labelled `label` (in the language now) was pressed, for [`on_press`]'s hook.
pub fn note_press(label: &str) {
    if let Some(hook) = PRESS_HOOK.get() {
        if let Some(name) = orbiters_i18n::control_name(label) {
            hook(name);
        }
    }
}

fn note_field_press() {
    if let (Some(hook), Some(name)) = (PRESS_HOOK.get(), FIELD.with(|f| f.get())) {
        hook(name);
    }
}

/// Where each control was drawn: rect, whether it took clicks, the pass it was drawn in.
#[derive(Clone, Default)]
struct Drawn(HashMap<String, (Rect, bool, u64)>);

fn drawn_id() -> Id {
    Id::unique("orbiters-widget-drawn")
}

/// From now on, buttons, icon buttons, segments and the record button note where they are drawn, for [`control`]
/// (for automated runs; nothing is noted otherwise).
pub fn watch_controls() {
    WATCH.store(true, Ordering::Relaxed);
}

/// Where the control labelled `label` (an icon button by its tooltip) was drawn in the last frame, and whether it
/// took clicks then.
pub fn control(ctx: &egui::Context, label: &str) -> Option<(Rect, bool)> {
    let pass = ctx.cumulative_pass_nr();
    let drawn = ctx.data(|d| d.get_temp::<Drawn>(drawn_id()))?;
    drawn.0.get(label).filter(|(_, _, p)| p + 1 >= pass).map(|(r, enabled, _)| (*r, *enabled))
}

/// Notes where a control labelled `label` was drawn, for [`control`] (an automated run only). It goes under its English names too: a run looks for a control by its English
/// name, and finds it in every language.
pub fn note_control(ctx: &egui::Context, label: &str, rect: Rect, enabled: bool) {
    if !WATCH.load(Ordering::Relaxed) {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let drawn = &mut d.get_temp_mut_or_default::<Drawn>(drawn_id()).0;
        for english in orbiters_i18n::english_names_of(label) {
            drawn.insert((*english).to_string(), (rect, enabled, pass));
        }
        drawn.insert(label.to_string(), (rect, enabled, pass));
    });
}

/// Paints a Phosphor glyph centred on `center` (or N4N0's face, [`icons::N4N0`], its mark, [`icons::N4N0_MARK`], or
/// Blender's logo, [`icons::BLENDER`]).
pub fn icon(painter: &Painter, center: Pos2, glyph: &str, size: f32, color: Color32) {
    if glyph == icons::N4N0 {
        return crate::n4n0_face::icon(painter, center, size, color);
    }
    if glyph == icons::N4N0_MARK {
        return crate::n4n0_logo::glyph(painter, center, size, color);
    }
    if glyph == icons::BLENDER {
        return crate::blender_logo::glyph(painter, center, size, color);
    }
    if glyph == icons::DISCORD_LOGO {
        return crate::discord_logo::glyph(painter, center, size, color);
    }
    painter.text(center, Align2::CENTER_CENTER, glyph, FontId::new(size, fonts::icons()), color);
}

/// Paints the filled variant of a Phosphor glyph (N4N0's face and mark are filled already).
pub fn icon_fill(painter: &Painter, center: Pos2, glyph: &str, size: f32, color: Color32) {
    if glyph == icons::N4N0 {
        return crate::n4n0_face::icon(painter, center, size, color);
    }
    if glyph == icons::N4N0_MARK {
        return crate::n4n0_logo::glyph(painter, center, size, color);
    }
    if glyph == icons::BLENDER {
        return crate::blender_logo::glyph(painter, center, size, color);
    }
    if glyph == icons::DISCORD_LOGO {
        return crate::discord_logo::glyph(painter, center, size, color);
    }
    painter.text(center, Align2::CENTER_CENTER, glyph, FontId::new(size, fonts::icons_fill()), color);
}

/// Paints text with its top-left (or other anchor) at `pos`; returns the text's rect.
pub fn text(painter: &Painter, pos: Pos2, anchor: Align2, txt: &Txt) -> Rect {
    let galley = painter.layout_job(txt.job(f32::INFINITY));
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, txt.color);
    rect
}

/// Paints one line of text cut with an ellipsis to `max_width`.
pub fn text_truncated(painter: &Painter, pos: Pos2, anchor: Align2, txt: &Txt, max_width: f32) -> Rect {
    let galley = painter.layout_job(txt.job_truncated(max_width.max(1.0)));
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, txt.color);
    rect
}

pub fn text_size(ui: &Ui, txt: &Txt) -> Vec2 {
    ui.painter().layout_job(txt.job(f32::INFINITY)).size()
}

/// True on the frame the primary button went down on this widget; also noted for [`take_press`].
pub fn pressed(resp: &Response) -> bool {
    let down = resp.is_pointer_button_down_on() && resp.ctx.input(|i| i.pointer.primary_pressed());
    if down {
        resp.ctx.data_mut(|d| d.insert_temp(press_id(), true));
    }
    down
}

/// Activate on pointer down, with keyboard/accessibility click fallback. Suppress the matching pointer-up click.
pub fn activated(resp: &Response) -> bool {
    let key = resp.id.with("activated-on-down");
    if pressed(resp) {
        resp.ctx.data_mut(|d| d.insert_temp(key, true));
        true
    } else if resp.clicked() {
        let down = resp.ctx.data_mut(|d| d.remove_temp::<bool>(key).unwrap_or(false));
        !down || !resp.ctx.input(|i| i.pointer.primary_released())
    } else {
        if !resp.ctx.input(|i| i.pointer.primary_down()) {
            resp.ctx.data_mut(|d| d.remove_temp::<bool>(key));
        }
        false
    }
}

/// Whether the click `resp` has this frame is a held key's: egui makes a click of the focused widget of every Enter or
/// Space, a held one's repeats included. Not a pointer's, not an accessibility tool's (even beside an unrelated repeat),
/// not a fresh press of the key.
fn held_key_click(resp: &Response) -> bool {
    if !resp.clicked() || resp.clicked_by(egui::PointerButton::Primary) {
        return false;
    }
    let id = resp.id;
    resp.ctx.input(|i| {
        let asked = i.has_accesskit_action_request(id, egui::accesskit::Action::Click);
        let fresh = i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Enter | egui::Key::Space, pressed: true, repeat: false, .. }));
        !asked && !fresh
    })
}

/// A click of the person's own: a pointer's, an accessibility tool's, or a fresh press of Enter or Space on the focused
/// widget, never a held key's repeats. For what is hard to take back or reaches someone else: send, approve, delete, sign
/// out, share, invite, start the game. (Also [`Button::confirm`].)
pub fn activated_fresh(resp: &Response) -> bool {
    resp.clicked() && !held_key_click(resp)
}

/// [`activated`] (on pointer down) as [`activated_fresh`]: never a held key's repeats.
pub fn pressed_fresh(resp: &Response) -> bool {
    activated(resp) && !held_key_click(resp)
}

fn press_id() -> egui::Id {
    egui::Id::unique("orbiters-widget-press")
}

/// Whether a control was pressed since the last call (the VR overlay answers presses with a click sound).
pub fn take_press(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(press_id()).unwrap_or(false))
}

/// Status colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Danger,
    Off,
    Rec,
    Info,
}

impl Tone {
    pub fn color(self) -> Color32 {
        match self {
            Tone::Ok => theme::OK,
            Tone::Warn => theme::WARN,
            Tone::Danger => theme::DANGER,
            Tone::Off => theme::FG_4,
            Tone::Rec => theme::REC,
            Tone::Info => theme::CYAN,
        }
    }
}

/// A frame with squircle corners (the website's panels and cards), `radius` as for a circular corner.
pub fn frame(painter: &Painter, rect: Rect, radius: f32, fill: Color32) {
    paint::fill_squircle(painter, rect, theme::squircle(radius), fill);
}

/// A flat card behind `rect`.
pub fn card(painter: &Painter, rect: Rect, radius: f32) {
    frame(painter, rect, radius, theme::surface());
}

/// A raised panel (popovers, toasts, dialogs) with the website's soft shadow.
pub fn panel(painter: &Painter, rect: Rect, radius: f32) {
    paint::shadow(painter, rect, radius, vec2(0.0, 12.0), 30.0, -8.0, theme::black(0.35));
    frame(painter, rect, radius, theme::surface_strong());
    paint::stroke_squircle(painter, rect, theme::squircle(radius), theme::hairline());
}

/// Pulsing status dot (10 points), the pulse ring only while live.
pub fn status_dot(painter: &Painter, center: Pos2, tone: Tone, pulse: bool) {
    let c = tone.color();
    if pulse && anim::is_live(painter.ctx()) {
        let t = anim::clock(painter.ctx(), 30.0);
        let k = (t % 1.0) as f32;
        let e = anim::ease::out_cubic(k);
        painter.circle_filled(center, 5.0 * (1.0 + e), theme::alpha(c, 0.6 * (1.0 - e)));
    }
    painter.circle_filled(center, 5.0, c);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Glass,
    /// Going ahead despite a warning (sharing something flagged explicit): amber, like [`Variant::Danger`] is red.
    Warn,
    /// The one way out of something running, which has to be found at a glance: solid red.
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

/// A pill button. Clicks on release (Unity's and the platform's usual behaviour); the press shows at once.
pub struct Button<'a> {
    label: &'a str,
    icon: Option<&'a str>,
    variant: Variant,
    size: Size,
    enabled: bool,
    width: Option<f32>,
    hint: Option<&'a str>,
    id_salt: Option<IdSalt>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { label, icon: None, variant: Variant::Secondary, size: Size::Md, enabled: true, width: None, hint: None, id_salt: None }
    }

    /// [`Button::show`] for what is hard to take back or reaches someone else: true on a press of the person's own
    /// ([`activated_fresh`]).
    pub fn confirm(self, ui: &mut Ui) -> bool {
        activated_fresh(&self.show(ui))
    }

    /// [`Button::confirm`] in a given rect, as [`Button::show_at`].
    pub fn confirm_at(self, ui: &mut Ui, rect: Rect) -> bool {
        activated_fresh(&self.show_at(ui, rect))
    }

    /// What tells this button from others with the same label and icon in one place (rows of a list), for
    /// [`Button::show_at`]; also keeps a button whose label changes (Hold to talk, Listening…) the same button.
    pub fn id_salt(mut self, salt: impl egui::AsIdSalt) -> Self {
        self.id_salt = Some(IdSalt::new(salt));
        self
    }

    pub fn icon(mut self, glyph: &'a str) -> Self {
        self.icon = Some(glyph);
        self
    }

    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }

    pub fn size(mut self, s: Size) -> Self {
        self.size = s;
        self
    }

    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn hint(mut self, h: &'a str) -> Self {
        self.hint = Some(h);
        self
    }

    fn metrics(&self) -> (f32, f32, f32, f32, f32) {
        // height, padding, text size, gap, icon size
        match self.size {
            Size::Sm => (32.0, 14.0, 12.5, 6.0, 14.0),
            Size::Md => (40.0, 18.0, 13.5, 8.0, 15.0),
            Size::Lg => (48.0, 24.0, 15.0, 10.0, 17.0),
        }
    }

    pub fn desired_width(&self, ui: &Ui) -> f32 {
        let (_, pad, size, gap, icon_size) = self.metrics();
        let text_w = if self.label.is_empty() { 0.0 } else { text_size(ui, &Txt::new(self.label, size).weight(weight::MEDIUM)).x };
        let icon_w = if self.icon.is_some() { icon_size + if self.label.is_empty() { 0.0 } else { gap } } else { 0.0 };
        self.width.unwrap_or(text_w + icon_w + pad * 2.0)
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (h, _, size, gap, icon_size) = self.metrics();
        let w = self.desired_width(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), if self.enabled { Sense::click() } else { Sense::hover() });
        note_control(ui.ctx(), self.label, rect, self.enabled);
        mark_control(orbiters_i18n::english_of(self.label), "button", rect);
        if resp.clicked() {
            note_press(self.label);
        }
        self.paint(ui, rect, &resp, size, gap, icon_size);
        let resp = if self.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        match self.hint {
            Some(hint) => resp.on_hover_text(hint),
            None => resp,
        }
    }

    /// Shows the button in a given rect (for layouts that place it themselves). Its id comes from its label's English and its icon
    /// (or [`Button::id_salt`]), never from where it is: a button that moves (scrolling, an entrance, a scale-in)
    /// stays one widget, so a press on it still clicks and no state is left behind at every position it passed; nor from its
    /// language, so a language change while it is held or open behind a popup leaves it the same button.
    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let (_, _, size, gap, icon_size) = self.metrics();
        let key = self.id_salt.unwrap_or_else(|| IdSalt::new((orbiters_i18n::english_of(self.label), self.icon)));
        let resp = ui.interact(rect, ui.scope_id().with("button").with_salt(key), if self.enabled { Sense::click() } else { Sense::hover() });
        if self.enabled {
            pressed(&resp);
        }
        note_control(ui.ctx(), self.label, rect, self.enabled);
        mark_control(orbiters_i18n::english_of(self.label), "button", rect);
        if resp.clicked() {
            note_press(self.label);
        }
        self.paint(ui, rect, &resp, size, gap, icon_size);
        let resp = if self.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        match self.hint {
            Some(hint) => resp.on_hover_text(hint),
            None => resp,
        }
    }

    fn paint(&self, ui: &Ui, rect: Rect, resp: &Response, size: f32, gap: f32, icon_size: f32) {
        let ctx = ui.ctx();
        let hover = anim::toggle(ctx, resp.id.with("hover"), self.enabled && resp.hovered(), Spring::SNAPPY);
        let press = anim::toggle(ctx, resp.id.with("press"), self.enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
        // A dialog landing back on it bounces it.
        let scale = (1.0 - 0.04 * press) * crate::morph::bounce(ctx, rect);
        let bg = Rect::from_center_size(rect.center(), rect.size() * scale);
        let mut painter = ui.painter().clone();
        if !self.enabled {
            // On top of the fade of what it sits in (a card's entrance), not instead of it.
            painter.multiply_opacity(0.4);
        }
        let r = bg.height() * 0.5;
        let fg = match self.variant {
            Variant::Primary => {
                painter.rect_filled(bg, theme::radius(r), theme::mix(theme::ACCENT, theme::ACCENT_HOVER, hover));
                theme::ON_ACCENT
            }
            Variant::Secondary => {
                painter.rect_filled(bg, theme::radius(r), theme::mix(theme::control(), theme::INK_600, hover));
                theme::FG
            }
            Variant::Ghost => {
                painter.rect_filled(bg, theme::radius(r), theme::white(0.06 * hover));
                theme::mix(theme::FG_2, theme::FG, hover)
            }
            Variant::Danger => {
                painter.rect_filled(bg, theme::radius(r), theme::alpha(theme::DANGER, 0.15 + 0.07 * hover));
                theme::DANGER
            }
            Variant::Stop => {
                painter.rect_filled(bg, theme::radius(r), theme::mix(theme::DANGER, theme::FG, 0.12 * hover));
                theme::INK_900
            }
            Variant::Glass => {
                painter.rect_filled(bg, theme::radius(r), theme::mix(theme::surface_strong(), theme::INK_700, hover));
                theme::FG
            }
            Variant::Warn => {
                painter.rect_filled(bg, theme::radius(r), theme::alpha(theme::WARN, 0.16 + 0.08 * hover));
                theme::WARN
            }
        };
        let txt = Txt::new(self.label, size).weight(weight::MEDIUM).color(fg);
        let text_w = if self.label.is_empty() { 0.0 } else { text_size(ui, &txt).x };
        let icon_w = if self.icon.is_some() { icon_size + if self.label.is_empty() { 0.0 } else { gap } } else { 0.0 };
        let mut x = bg.center().x - (text_w + icon_w) * 0.5;
        if let Some(glyph) = self.icon {
            icon(&painter, pos2(x + icon_size * 0.5, bg.center().y), glyph, icon_size, fg);
            x += icon_w;
        }
        if !self.label.is_empty() {
            text(&painter, pos2(x, bg.center().y), Align2::LEFT_CENTER, &txt);
        }
    }
}

/// Round icon button (36 points), lighter on hover, deeper press. Its label is the English text: it names the button for an automated run
/// ([`control`]) and for its id, and the tooltip shows it in the language now. A label written as `mark("Close")` is a text to translate.
pub struct IconButton<'a> {
    glyph: &'a str,
    label: &'a str,
    size: f32,
    icon_size: f32,
    danger: bool,
    backdrop: Option<Color32>,
    id_salt: Option<IdSalt>,
}

impl<'a> IconButton<'a> {
    pub fn new(glyph: &'a str, label: &'a str) -> Self {
        Self { glyph, label, size: 36.0, icon_size: 17.0, danger: false, backdrop: None, id_salt: None }
    }

    /// What tells this button from others with the same icon and label in one place, for [`IconButton::show_at`].
    pub fn id_salt(mut self, salt: impl egui::AsIdSalt) -> Self {
        self.id_salt = Some(IdSalt::new(salt));
        self
    }

    pub fn size(mut self, size: f32, icon_size: f32) -> Self {
        self.size = size;
        self.icon_size = icon_size;
        self
    }

    /// Red on hover (destructive actions).
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// A resting background (over pictures).
    pub fn backdrop(mut self, c: Color32) -> Self {
        self.backdrop = Some(c);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::click());
        note_control(ui.ctx(), self.label, rect, true);
        mark_control(orbiters_i18n::english_of(self.label), "icon-button", rect);
        if resp.clicked() {
            note_press(self.label);
        }
        self.paint(ui, rect, &resp);
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(orbiters_i18n::tr(self.label))
    }

    /// Shows the button in a given rect; its id comes from its icon and label (or [`IconButton::id_salt`]), never
    /// from where it is.
    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let key = self.id_salt.unwrap_or_else(|| IdSalt::new((self.glyph, orbiters_i18n::english_of(self.label))));
        let resp = ui.interact(rect, ui.scope_id().with("icon-button").with_salt(key), Sense::click());
        pressed(&resp);
        note_control(ui.ctx(), self.label, rect, true);
        mark_control(orbiters_i18n::english_of(self.label), "icon-button", rect);
        if resp.clicked() {
            note_press(self.label);
        }
        self.paint(ui, rect, &resp);
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(orbiters_i18n::tr(self.label))
    }

    fn paint(&self, ui: &Ui, rect: Rect, resp: &Response) {
        let ctx = ui.ctx();
        let hover = anim::toggle(ctx, resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let press = anim::toggle(ctx, resp.id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.12 * press) * crate::morph::bounce(ctx, rect));
        let p = ui.painter();
        if let Some(c) = self.backdrop {
            p.circle_filled(r.center(), r.width() * 0.5, c);
        }
        let (bg, fg) = if self.danger {
            (theme::alpha(theme::DANGER, 0.15 * hover), theme::mix(theme::FG_2, theme::DANGER, hover))
        } else {
            (theme::white(0.08 * hover), theme::mix(theme::FG_2, theme::FG, hover))
        };
        p.circle_filled(r.center(), r.width() * 0.5, bg);
        icon(p, r.center(), self.glyph, self.icon_size, fg);
    }
}

/// One option of a [`segmented`] control.
pub struct SegOption<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub hint: Option<&'a str>,
    pub disabled: bool,
    pub badge: Option<&'a str>,
}

impl<'a, T> SegOption<'a, T> {
    pub fn new(value: T, label: &'a str) -> Self {
        Self { value, label, hint: None, disabled: false, badge: None }
    }

    pub fn hint(mut self, h: &'a str) -> Self {
        self.hint = Some(h);
        self
    }

    pub fn disabled(mut self, badge: &'a str) -> Self {
        self.disabled = true;
        self.badge = Some(badge);
        self
    }
}

/// The width of each option of a [`segmented`] control, as it lays them out.
fn segment_widths<T>(ui: &Ui, options: &[SegOption<'_, T>], small: bool) -> Vec<f32> {
    let (pad_x, size) = if small { (12.0, 12.0) } else { (14.0, 13.0) };
    let badge_txt = |b: &str| Txt::new(b.to_uppercase(), 9.5).weight(weight::SEMIBOLD).tracking(0.04).color(theme::FG_2);
    options
        .iter()
        .map(|o| {
            let t = text_size(ui, &Txt::new(o.label, size).weight(weight::MEDIUM)).x;
            let b = o.badge.map(|b| text_size(ui, &badge_txt(b)).x + 12.0 + 6.0).unwrap_or(0.0);
            t + b + pad_x * 2.0
        })
        .collect()
}

/// The whole width a [`segmented`] control takes with these options (its track), in this language: for a row to know
/// whether it fits before it is drawn.
pub fn segmented_width<T>(ui: &Ui, options: &[SegOption<'_, T>], small: bool) -> f32 {
    segment_widths(ui, options, small).iter().sum::<f32>() + 8.0
}

/// The options' widths fitted to `avail` (Night PG2-9: a control wider than its column pushed the column out, and
/// everything laid out after it went past the panel's edge): as they are when they fit; else their padding shrinks, down
/// to 6 points a side; then they share what there is in proportion (their labels are then cut).
fn fit_segments(mut widths: Vec<f32>, pad_x: f32, avail: f32) -> Vec<f32> {
    let natural = widths.iter().sum::<f32>() + 8.0;
    if widths.is_empty() || natural <= avail || !avail.is_finite() {
        return widths;
    }
    let n = widths.len() as f32;
    let less = (natural - avail).min(n * 2.0 * (pad_x - 6.0).max(0.0)) / n;
    for w in &mut widths {
        *w -= less;
    }
    let inner = widths.iter().sum::<f32>();
    if inner + 8.0 > avail {
        let k = ((avail - 8.0).max(0.0) / inner).min(1.0);
        for w in &mut widths {
            *w *= k;
        }
    }
    widths
}

/// Segmented control whose highlight slides between options with a bouncy spring. Returns true when the value
/// changed (on press). Never wider than the room it is given ([`fit_segments`]).
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut T, options: &[SegOption<'_, T>], small: bool) -> bool {
    let key = salt_key(&id_salt);
    let id = ui.scope_id().with(id_salt);
    let (item_h, size) = if small { (28.0, 12.0) } else { (32.0, 13.0) };
    let badge_txt = |b: &str| Txt::new(b.to_uppercase(), 9.5).weight(weight::SEMIBOLD).tracking(0.04).color(theme::FG_2);
    let widths = fit_segments(segment_widths(ui, options, small), if small { 12.0 } else { 14.0 }, ui.available_width());
    let track_w = widths.iter().sum::<f32>() + 8.0;
    let (track, _) = ui.allocate_exact_size(vec2(track_w, item_h + 8.0), Sense::hover());
    mark_control(&key, "segments", track);
    let painter = ui.painter();
    painter.rect_filled(track, theme::radius(track.height() * 0.5), theme::surface_soft());

    let mut rects = Vec::with_capacity(options.len());
    let mut x = track.left() + 4.0;
    for w in &widths {
        rects.push(Rect::from_min_size(pos2(x, track.top() + 4.0), vec2(*w, item_h)));
        x += w;
    }
    let mut changed = false;
    let mut responses = Vec::with_capacity(options.len());
    for (i, o) in options.iter().enumerate() {
        let resp = ui.interact(rects[i], id.with(i), if o.disabled { Sense::hover() } else { Sense::click() });
        if !o.disabled && pressed(&resp) && *value != o.value {
            *value = o.value;
            changed = true;
            note_press(o.label);
        }
        note_control(ui.ctx(), o.label, rects[i], !o.disabled);
        responses.push(resp);
    }

    // Highlight under the active option, sprung in track-local coordinates (so scrolling never animates). The bounce
    // overshoots by a fifth of the move: it stays inside the track.
    if let Some(active) = options.iter().position(|o| o.value == *value) {
        let r = rects[active];
        let [l, w] = anim::spring(ui.ctx(), id.with("pill"), [r.left() - track.left(), r.width()], Spring::BOUNCY);
        let w = w.clamp(0.0, (track.width() - 8.0).max(0.0));
        let l = l.clamp(4.0, (track.width() - 4.0 - w).max(4.0));
        let pill = Rect::from_min_size(pos2(track.left() + l, r.top()), vec2(w, r.height()));
        painter.rect_filled(pill, theme::radius(pill.height() * 0.5), theme::INK_600);
    }

    for (i, o) in options.iter().enumerate() {
        let active = o.value == *value;
        let hovered = responses[i].hovered() && !o.disabled;
        let target = if active {
            Color32::WHITE
        } else if hovered {
            theme::FG_2
        } else {
            theme::FG_3
        };
        let color = anim::color(ui.ctx(), id.with(("c", i)), target, Spring::SNAPPY);
        let mut p = ui.painter().clone();
        if o.disabled {
            p.multiply_opacity(0.45);
        }
        let label = Txt::new(o.label, size).weight(weight::MEDIUM).color(color);
        let lw = text_size(ui, &label).x;
        let bw = o.badge.map(|b| text_size(ui, &badge_txt(b)).x + 12.0 + 6.0).unwrap_or(0.0);
        let room = rects[i].width() - 12.0;
        let x0 = if lw + bw <= room {
            let x0 = rects[i].center().x - (lw + bw) * 0.5;
            text(&p, pos2(x0, rects[i].center().y), Align2::LEFT_CENTER, &label);
            x0
        } else {
            // Squeezed into a column narrower than the labels: cut, from the left padding.
            let x0 = rects[i].left() + 6.0;
            text_truncated(&p, pos2(x0, rects[i].center().y), Align2::LEFT_CENTER, &label, (room - bw).max(0.0));
            x0
        };
        let lw = lw.min((room - bw).max(0.0));
        if let Some(b) = o.badge {
            let bt = badge_txt(b);
            let bs = text_size(ui, &bt);
            let br = Rect::from_min_size(pos2(x0 + lw + 6.0, rects[i].center().y - 8.0), vec2(bs.x + 12.0, 16.0));
            p.rect_filled(br, theme::radius(8.0), theme::white(0.1));
            text(&p, br.center(), Align2::CENTER_CENTER, &bt);
        }
    }
    for (i, resp) in responses.into_iter().enumerate() {
        let resp = if options[i].disabled { resp.on_hover_cursor(CursorIcon::NotAllowed) } else { resp.on_hover_cursor(CursorIcon::PointingHand) };
        if let Some(h) = options[i].hint {
            resp.on_hover_text(h);
        }
    }
    changed
}

#[cfg(test)]
mod segmented_tests {
    use super::fit_segments;

    /// Night PG2-9: a control wider than its column is fitted to it; one that fits is as it was.
    #[test]
    fn a_segmented_control_is_never_wider_than_its_room() {
        let natural = vec![60.0, 80.0, 70.0, 64.0];
        assert_eq!(fit_segments(natural.clone(), 14.0, 400.0), natural, "it fits: unchanged");
        let tighter = fit_segments(natural.clone(), 14.0, 260.0);
        assert!((tighter.iter().sum::<f32>() + 8.0 - 260.0).abs() < 0.01, "padding shrunk to fit: {tighter:?}");
        let shared = fit_segments(natural.clone(), 14.0, 150.0);
        assert!(shared.iter().sum::<f32>() + 8.0 <= 150.01, "{shared:?}");
        assert!(shared.windows(2).all(|w| w[0] > 0.0 && w[1] > 0.0));
    }
}

/// Switch: the knob springs across, the track turns Orbiters green. Acts on press.
pub fn toggle(ui: &mut Ui, id_salt: impl egui::AsIdSalt, on: &mut bool) -> bool {
    mark_control(&salt_key(&id_salt), "toggle", Rect::NOTHING);
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 28.0), Sense::hover());
    set_last_mark(rect);
    let resp = ui.interact(rect, id, Sense::click());
    let mut changed = false;
    if pressed(&resp) {
        *on = !*on;
        changed = true;
        note_field_press();
    }
    let ctx = ui.ctx();
    let k = anim::toggle(ctx, id.with("k"), *on, Spring::BOUNCY);
    let fill = anim::toggle(ctx, id.with("f"), *on, Spring::SOFT).clamp(0.0, 1.0);
    let press = anim::toggle(ctx, id.with("p"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let track = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.08 * press));
    let p = ui.painter();
    p.rect_filled(track, theme::radius(track.height() * 0.5), theme::mix(theme::INK_600, theme::ACCENT, fill));
    let knob_r = 10.0 * (1.0 - 0.08 * press);
    let x = track.left() + 4.0 + knob_r + k * (track.width() - 8.0 - knob_r * 2.0);
    let c = pos2(x, track.center().y);
    p.circle_filled(c, knob_r, Color32::WHITE);
    resp.on_hover_cursor(CursorIcon::PointingHand);
    changed
}

/// Slider with a green fill and a springy knob; the value label sits on the right.
pub fn slider(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    format: impl Fn(f32) -> String,
) -> bool {
    mark_control(&salt_key(&id_salt), "slider", Rect::NOTHING);
    let id = ui.scope_id().with(id_salt);
    let (min, max) = (*range.start(), *range.end());
    let (rect, _) = ui.allocate_exact_size(vec2(192.0 + 12.0 + 80.0, 28.0), Sense::hover());
    set_last_mark(rect);
    let track_rect = Rect::from_min_size(rect.min, vec2(192.0, 28.0));
    let resp = ui.interact(track_rect, id, Sense::click_and_drag());
    if resp.drag_stopped() || (resp.clicked() && !resp.dragged()) {
        note_field_press();
    }
    let mut changed = false;
    if let (true, Some(p)) = (resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
        let t = ((p.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
        let mut v = min + t * (max - min);
        if step > 0.0 {
            v = (v / step).round() * step;
        }
        v = v.clamp(min, max);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            changed = true;
        }
    }
    let t = if max > min { ((*value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let ctx = ui.ctx();
    let fill_t = anim::value(ctx, id.with("fill"), t, Spring::SNAPPY).clamp(0.0, 1.0);
    // The knob bounces, but never off the bar's ends (into the value's label).
    let knob_t = anim::value(ctx, id.with("knob"), t, Spring::BOUNCY).clamp(0.0, 1.0);
    let hover = anim::toggle(ctx, id.with("hover"), resp.hovered() || resp.dragged(), Spring::SNAPPY);
    let p = ui.painter();
    let bar = Rect::from_center_size(track_rect.center(), vec2(track_rect.width(), 6.0));
    p.rect_filled(bar, theme::radius(3.0), theme::white(0.09));
    if fill_t > 0.0 {
        let fill = Rect::from_min_size(bar.min, vec2((bar.width() * fill_t).max(6.0), bar.height()));
        p.rect_filled(fill, theme::radius(3.0), theme::ACCENT);
    }
    let c = pos2(bar.left() + bar.width() * knob_t, bar.center().y);
    let r = 9.0 + 1.0 * hover;
    p.circle_filled(c, r, Color32::WHITE);
    let label = Txt::new(format(*value), 12.5).color(theme::FG_2);
    text(p, pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, &label);
    resp.on_hover_cursor(CursorIcon::PointingHand);
    changed
}

/// Single-line text field: dark well, hairline, indigo focus ring.
pub fn text_field(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, hint: &str, width: f32, mono: bool) -> Response {
    let key = salt_key(&id_salt);
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    mark_control(&key, "text-field", rect);
    field_frame(ui, rect, id, value, hint, Well { mono, leading: None, radius: 12.0, clear: false })
}

/// Search field: pill with a magnifier.
pub fn search_field(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, hint: &str, width: f32) -> Response {
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    field_frame(ui, rect, id, value, hint, Well { mono: false, leading: Some(icons::MAGNIFYING_GLASS), radius: 20.0, clear: false })
}

/// A [`search_field`] with an × at its end while something is typed: it empties the field (a change, like typing) and
/// leaves the focus there to type the next thing.
pub fn search_field_clearable(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, hint: &str, width: f32) -> Response {
    let key = salt_key(&id_salt);
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    mark_control(&key, "search-field", rect);
    field_frame(ui, rect, id, value, hint, Well { mono: false, leading: Some(icons::MAGNIFYING_GLASS), radius: 20.0, clear: true })
}

/// Look of a text well.
struct Well<'a> {
    mono: bool,
    leading: Option<&'a str>,
    radius: f32,
    /// Room for an × at its end, shown while there is text.
    clear: bool,
}

fn field_frame(ui: &mut Ui, rect: Rect, id: Id, value: &mut String, hint: &str, well: Well<'_>) -> Response {
    let Well { mono, leading, radius, clear } = well;
    let focused = ui.memory(|m| m.has_focus(id));
    let ring = anim::toggle(ui.ctx(), id.with("ring"), focused, Spring::SNAPPY);
    let p = ui.painter().clone();
    // The website's input: a flat zinc field, a green ring while focused.
    p.rect_filled(rect, theme::radius(radius), theme::INK_750);
    if ring > 0.01 {
        p.rect_stroke(rect, theme::radius(radius), Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.8 * ring)), StrokeKind::Outside);
    }
    let mut inner = rect.shrink2(vec2(12.0, 0.0));
    if let Some(glyph) = leading {
        icon(&p, pos2(rect.left() + 18.0, rect.center().y), glyph, 15.0, theme::FG_4);
        inner.min.x = rect.left() + 34.0;
    }
    if clear {
        inner.max.x = rect.right() - 38.0;
    }
    let font = if mono { FontId::new(12.0, egui::FontFamily::Monospace) } else { theme::font(13.0, weight::REGULAR) };
    let edit = egui::TextEdit::singleline(value)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(egui::RichText::new(hint.to_string()).color(theme::FG_4).font(font.clone()))
        .font(font)
        .text_color(theme::FG)
        .desired_width(inner.width())
        .margin(egui::Margin::ZERO)
        .align(egui::Align2::LEFT_CENTER);
    let mut resp = ui.put(inner, edit);
    if clear && !value.is_empty() {
        let x = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(28.0, 28.0));
        if activated(&IconButton::new(icons::X, orbiters_i18n::mark("Clear")).size(28.0, 13.0).id_salt(id.with("clear")).show_at(ui, x)) {
            value.clear();
            resp.mark_changed();
            resp.request_focus();
        }
    }
    resp
}

/// Status chip of the capture page: tinted icon tile, caption and a value that slides when it changes.
pub fn status_chip(ui: &mut Ui, rect: Rect, id: Id, glyph: &str, label: &str, value: &str, tone: Tone) {
    let p = ui.painter();
    frame(p, rect, 16.0, theme::surface());
    let tile = Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - 18.0), vec2(36.0, 36.0));
    let (bg, fg) = match tone {
        Tone::Ok => (theme::alpha(theme::OK, 0.1), theme::OK),
        Tone::Warn => (theme::alpha(theme::WARN, 0.1), theme::WARN),
        Tone::Danger => (theme::alpha(theme::DANGER, 0.1), theme::DANGER),
        Tone::Rec => (theme::alpha(theme::REC, 0.12), theme::REC),
        Tone::Info => (theme::alpha(theme::CYAN, 0.1), theme::CYAN),
        Tone::Off => (theme::white(0.05), theme::FG_4),
    };
    let bg = anim::color(ui.ctx(), id.with("bg"), bg, Spring::SOFT);
    let fg = anim::color(ui.ctx(), id.with("fg"), fg, Spring::SOFT);
    p.rect_filled(tile, theme::radius(12.0), bg);
    icon(p, tile.center(), glyph, 16.0, fg);
    let x = tile.right() + 12.0;
    let max_w = rect.right() - x - 12.0;
    text(p, pos2(x, rect.center().y - 9.0), Align2::LEFT_CENTER, &Txt::caps(label));
    sliding_value(
        ui,
        id.with("value"),
        pos2(x, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        value,
        &Txt::new("", 13.0).weight(weight::MEDIUM).color(theme::FG),
        max_w,
    );
}

#[derive(Clone)]
struct Slide {
    current: String,
    previous: Option<String>,
    since: f64,
}

/// A line of text that slides up and fades into its new value when it changes (anchored at `pos`).
pub fn sliding_value(ui: &Ui, id: Id, pos: Pos2, anchor: Align2, value: &str, style: &Txt, max_w: f32) {
    let ctx = ui.ctx();
    let now = anim::time(ctx);
    let mut s = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(id, || Slide { current: value.to_string(), previous: None, since: -10.0 }).clone());
    if s.current != value {
        s.previous = Some(std::mem::replace(&mut s.current, value.to_string()));
        s.since = now;
        ctx.data_mut(|d| d.insert_temp(id, s.clone()));
    }
    let dur = 0.18;
    let k = if anim::is_live(ctx) { anim::ease::out_cubic((anim::since(ctx, s.since, dur) / dur) as f32) } else { 1.0 };
    let p = ui.painter();
    if let (Some(prev), true) = (&s.previous, k < 1.0) {
        let t = Txt { text: prev.clone(), color: theme::alpha(style.color, 1.0 - k), ..style.clone() };
        text_truncated(p, pos + vec2(0.0, -6.0 * k), anchor, &t, max_w);
    }
    let t = Txt { text: s.current.clone(), color: theme::alpha(style.color, k), ..style.clone() };
    text_truncated(p, pos + vec2(0.0, 6.0 * (1.0 - k)), anchor, &t, max_w);
}

/// A number easing towards `value` (the web's AnimatedNumber); settles within display precision.
pub fn animated_number(ui: &Ui, id: Id, value: f64) -> f64 {
    let v = value as f32;
    let eps = (v.abs() * 2e-4).max(0.01);
    anim::spring_eps(ui.ctx(), id, [v], [v], Spring::NUMBER, eps)[0] as f64
}

/// What a [`stat_tile`] shows.
pub struct Stat<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    pub number: &'a str,
    pub sub: &'a str,
    /// Icon colour (a status), or the quiet default.
    pub accent: Option<Color32>,
}

/// Stat tile: caption and icon, a big counting number, a sub line; a step lighter on hover.
pub fn stat_tile(ui: &mut Ui, rect: Rect, id: Id, stat: &Stat<'_>) {
    let Stat { glyph, label, number, sub, accent } = *stat;
    let resp = ui.interact(rect, id, Sense::hover());
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SOFT);
    let r = rect;
    let p = ui.painter();
    frame(p, r, theme::TILE_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));
    text(p, r.left_top() + vec2(16.0, 22.0), Align2::LEFT_CENTER, &Txt::caps(label));
    icon(p, r.right_top() + vec2(-24.0, 22.0), glyph, 16.0, accent.unwrap_or(theme::FG_4));
    text_truncated(
        p,
        pos2(r.left() + 16.0, r.center().y + 2.0),
        Align2::LEFT_CENTER,
        &Txt::new(number, 28.0).weight(weight::SEMIBOLD).tracking(-0.01),
        r.width() - 32.0,
    );
    text_truncated(p, pos2(r.left() + 16.0, r.bottom() - 20.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.0).color(theme::FG_3), r.width() - 32.0);
}

/// Small rounded tag with an optional icon (counts on session cards).
pub fn pill(ui: &mut Ui, glyph: Option<&str>, label: &str) -> Response {
    let t = Txt::new(label, 11.5).color(theme::FG_2);
    let ts = text_size(ui, &t);
    let w = ts.x + 16.0 + if glyph.is_some() { 16.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(10.0), theme::white(0.06));
    let mut x = rect.left() + 8.0;
    if let Some(g) = glyph {
        icon(p, pos2(x + 6.0, rect.center().y), g, 12.0, theme::FG_4);
        x += 16.0;
    }
    text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, &t);
    resp
}

/// Paints a small tag with its left edge centred on `left_center`; returns its width.
pub fn pill_at(painter: &Painter, ui: &Ui, left_center: Pos2, glyph: Option<&str>, label: &str) -> f32 {
    let t = Txt::new(label, 11.5).color(theme::FG_2);
    let ts = text_size(ui, &t);
    let w = ts.x + 16.0 + if glyph.is_some() { 16.0 } else { 0.0 };
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 10.0), vec2(w, 20.0));
    painter.rect_filled(rect, theme::radius(10.0), theme::white(0.06));
    let mut x = rect.left() + 8.0;
    if let Some(g) = glyph {
        icon(painter, pos2(x + 6.0, rect.center().y), g, 12.0, theme::FG_4);
        x += 16.0;
    }
    text(painter, pos2(x, rect.center().y), Align2::LEFT_CENTER, &t);
    w
}

/// Section heading of a settings card: a tinted icon tile (as the website's forms), title and subtitle.
pub fn section_header(ui: &mut Ui, glyph: &str, title: &str, subtitle: Option<&str>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let p = ui.painter();
    let tile = Rect::from_min_size(pos2(rect.left(), rect.center().y - 18.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(11.0), theme::alpha(theme::ACCENT, 0.12));
    icon(p, tile.center(), glyph, 17.0, theme::ACCENT);
    let x = tile.right() + 12.0;
    match subtitle {
        Some(sub) => {
            text(p, pos2(x, rect.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(title, 15.5).weight(weight::SEMIBOLD));
            text_truncated(p, pos2(x, rect.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.0).color(theme::FG_3), rect.right() - x);
        }
        None => {
            text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, &Txt::new(title, 15.5).weight(weight::SEMIBOLD));
        }
    }
}

/// A settings row: label and wrapped description on the left, the control on the right. `control_width` is the
/// width the control needs.
pub fn field(ui: &mut Ui, label: &str, description: Option<&str>, control_width: f32, control: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let text_w = (width - control_width - 24.0).max(120.0);
    let label_txt = Txt::new(label, 13.5).weight(weight::MEDIUM);
    let label_g = ui.painter().layout_job(label_txt.job(text_w));
    let desc_g = description.map(|d| ui.painter().layout_job(Txt::new(d, 12.0).color(theme::FG_3).line_height(18.0).job(text_w)));
    let text_h = label_g.size().y + desc_g.as_ref().map(|g| g.size().y + 2.0).unwrap_or(0.0);
    let h = text_h.max(36.0) + 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    ROWS.with(|r| {
        if let Some(rows) = r.borrow_mut().as_mut() {
            let slot = ui.painter().add(egui::Shape::Noop);
            let control = Rect::from_min_max(pos2(rect.right() - control_width, rect.top()), rect.max);
            rows.push(FieldRow { label: label.to_owned(), rect, control, text_width: width - control_width - 24.0, behind: (ui.painter().clone(), slot) });
        }
    });
    let p = ui.painter();
    let mut y = rect.center().y - text_h * 0.5;
    p.galley(pos2(rect.left(), y), label_g.clone(), theme::FG);
    y += label_g.size().y + 2.0;
    if let Some(g) = desc_g {
        p.galley(pos2(rect.left(), y), g, theme::FG_3);
    }
    let control_rect = Rect::from_min_max(pos2(rect.right() - control_width, rect.top()), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    // The row's label names its control's presses; the label lives only for this call, so it is the English of it that is kept (a catalog text is 'static).
    let named = PRESS_HOOK.get().and(orbiters_i18n::control_name(label));
    let was = FIELD.with(|f| f.replace(named));
    control(&mut child);
    FIELD.with(|f| f.set(was));
}

/// Hairline between rows.
pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, theme::white(0.05)));
}

/// Progress bar (storage usage): sprung width, green or a warning colour.
pub fn progress(ui: &mut Ui, id: Id, fraction: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(height * 0.5), theme::white(0.07));
    let f = anim::spring_from(ui.ctx(), id, [0.0], [fraction.clamp(0.0, 1.0)], Spring::SOFT)[0].clamp(0.0, 1.0);
    if f > 0.001 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(height), height));
        if fraction > 0.9 {
            p.rect_filled(fill, theme::radius(height * 0.5), theme::DANGER);
        } else if fraction > 0.7 {
            p.rect_filled(fill, theme::radius(height * 0.5), theme::WARN);
        } else {
            p.rect_filled(fill, theme::radius(height * 0.5), theme::ACCENT);
        }
    }
}

/// Opacity and vertical offset of a staggered entrance (`index` steps of 50 ms after `start`).
pub fn entrance(ui: &Ui, id: Id, start: f64, index: usize) -> (f32, f32) {
    let ctx = ui.ctx();
    let delay = 0.05 + index as f64 * 0.05;
    let elapsed = anim::time(ctx) - start;
    let target = if elapsed >= delay { 1.0 } else { 0.0 };
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let v = anim::spring_from(ctx, id, [0.0], [target], Spring::SOFT)[0];
    (v.clamp(0.0, 1.0), 18.0 * (1.0 - v))
}

/// A colour of its own for each name, for people without a picture (one of the theme's accents).
pub fn name_color(name: &str) -> Color32 {
    let palette = [theme::CYAN, theme::VIOLET, theme::INDIGO, theme::BLUE, theme::PURPLE, theme::ACCENT, theme::WARN, theme::REC];
    let hash = name.bytes().fold(2_166_136_261u32, |h, b| (h ^ b as u32).wrapping_mul(16_777_619));
    palette[hash as usize % palette.len()]
}

/// A person in the circle inscribed in `rect`: their picture (cropped to fill it), else their initial on their own
/// colour ([`name_color`]).
pub fn avatar(painter: &Painter, rect: Rect, picture: Option<&egui::TextureHandle>, name: &str) {
    match picture {
        Some(t) => {
            let size = t.size_vec2();
            let (ia, ta) = (size.x / size.y.max(1.0), rect.width() / rect.height().max(1.0));
            let (w, h) = if ta > ia { (1.0, ia / ta) } else { (ta / ia, 1.0) };
            paint::circle_texture(painter, rect, t.id(), Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h)), Color32::WHITE);
        }
        None => {
            let c = name_color(name);
            painter.circle_filled(rect.center(), rect.width().min(rect.height()) * 0.5, theme::alpha(c, 0.2));
            let initial = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
            text(painter, rect.center(), Align2::CENTER_CENTER, &Txt::new(initial, rect.height() * 0.42).weight(weight::SEMIBOLD).color(c));
        }
    }
}

/// A count on a control (requests waiting): a green pill centred on `center`; returns its rect.
pub fn count_badge(painter: &Painter, center: Pos2, count: usize) -> Rect {
    let t = Txt::new(if count > 99 { "99+".to_string() } else { count.to_string() }, 10.5).weight(weight::BOLD).color(theme::ON_ACCENT);
    let w = painter.layout_job(t.job(f32::INFINITY)).size().x;
    let r = Rect::from_center_size(center, vec2((w + 10.0).max(18.0), 18.0));
    painter.rect_filled(r, theme::radius(9.0), theme::ACCENT);
    text(painter, r.center(), Align2::CENTER_CENTER, &t);
    r
}

/// Height of a [`Chip`].
pub const CHIP_H: f32 = 32.0;

/// A rounded chip: a person (their picture or initial) or an icon, a label, then a check while it is chosen or an ×
/// that removes it. Choosing acts on press, like the other selection controls.
pub struct Chip<'a> {
    label: &'a str,
    glyph: Option<&'a str>,
    tone: Option<Color32>,
    person: Option<Option<&'a egui::TextureHandle>>,
    selected: bool,
    removable: bool,
    enabled: bool,
    id_salt: Option<IdSalt>,
}

/// What happened to a [`Chip`] this frame.
pub struct ChipResponse {
    pub response: Response,
    /// Pressed, anywhere but its ×.
    pub pressed: bool,
    /// Its × was pressed.
    pub removed: bool,
}

impl<'a> Chip<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { label, glyph: None, tone: None, person: None, selected: false, removable: false, enabled: true, id_salt: None }
    }

    pub fn icon(mut self, glyph: &'a str) -> Self {
        self.glyph = Some(glyph);
        self
    }

    /// The icon's colour (a status: green for public).
    pub fn tone(mut self, c: Color32) -> Self {
        self.tone = Some(c);
        self
    }

    /// A person's chip: their picture when there is one, else their initial.
    pub fn person(mut self, picture: Option<&'a egui::TextureHandle>) -> Self {
        self.person = Some(picture);
        self
    }

    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }

    /// An × at its end ([`ChipResponse::removed`]).
    pub fn removable(mut self, r: bool) -> Self {
        self.removable = r;
        self
    }

    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    /// What tells it from other chips with the same label in one place.
    pub fn id_salt(mut self, salt: impl egui::AsIdSalt) -> Self {
        self.id_salt = Some(IdSalt::new(salt));
        self
    }

    fn label_txt(&self) -> Txt {
        Txt::new(self.label, 12.5).weight(weight::MEDIUM)
    }

    fn lead(&self) -> f32 {
        if self.person.is_some() {
            36.0
        } else if self.glyph.is_some() {
            32.0
        } else {
            14.0
        }
    }

    fn trail(&self) -> f32 {
        if self.removable || self.selected {
            32.0
        } else {
            14.0
        }
    }

    /// Its width with its label whole.
    pub fn width(&self, ui: &Ui) -> f32 {
        self.lead() + text_size(ui, &self.label_txt()).x + self.trail()
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> ChipResponse {
        let key = self.id_salt.unwrap_or_else(|| IdSalt::new((self.label, self.glyph)));
        let id = ui.scope_id().with("chip").with_salt(key);
        let resp = ui.interact(rect, id, if self.enabled { Sense::click() } else { Sense::hover() });
        let x_zone = Rect::from_min_max(pos2(rect.right() - 30.0, rect.top()), rect.max);
        let over_x = self.removable && resp.interact_pointer_pos().or(resp.hover_pos()).is_some_and(|p| x_zone.contains(p));
        let act = self.enabled && activated(&resp);
        note_control(ui.ctx(), self.label, rect, self.enabled);
        let ctx = ui.ctx();
        let hover = anim::toggle(ctx, id.with("hover"), self.enabled && resp.hovered() && !over_x, Spring::SNAPPY);
        let x_hover = anim::toggle(ctx, id.with("x-hover"), self.enabled && resp.hovered() && over_x, Spring::SNAPPY);
        let press = anim::toggle(ctx, id.with("press"), self.enabled && resp.is_pointer_button_down_on() && !over_x, Spring::SNAPPY);
        let sel = anim::toggle(ctx, id.with("selected"), self.selected, Spring::BOUNCY);
        let k = sel.clamp(0.0, 1.0);
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.04 * press));
        let mut p = ui.painter().clone();
        if !self.enabled {
            p.multiply_opacity(0.45);
        }
        let idle = theme::mix(theme::control(), theme::INK_600, hover);
        let chosen = theme::alpha(theme::ACCENT, 0.16 + 0.06 * hover);
        p.rect_filled(r, theme::radius(r.height() * 0.5), theme::mix(idle, chosen, k));
        let fg = theme::mix(theme::mix(theme::FG_2, theme::FG, hover), theme::FG, k);
        if let Some(picture) = self.person {
            let a = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(24.0, 24.0));
            avatar(&p, a, picture, self.label);
            if k > 0.01 {
                p.circle_stroke(a.center(), 12.75, Stroke::new(1.5, theme::alpha(theme::ACCENT, k)));
            }
        } else if let Some(g) = self.glyph {
            icon(&p, pos2(r.left() + 19.0, r.center().y), g, 14.0, self.tone.unwrap_or(fg));
        }
        let label_max = r.width() - self.lead() - self.trail() + 2.0;
        text_truncated(&p, pos2(r.left() + self.lead(), r.center().y), Align2::LEFT_CENTER, &self.label_txt().color(fg), label_max.max(8.0));
        let end = pos2(r.right() - 16.0, r.center().y);
        if self.removable {
            p.circle_filled(end, 10.0, theme::white(0.1 * x_hover));
            icon(&p, end, icons::X, 12.0, theme::mix(theme::FG_3, theme::FG, x_hover));
        } else if k > 0.01 {
            let mut cp = p.clone();
            cp.multiply_opacity(k);
            icon(&cp, end + vec2(0.0, 4.0 * (1.0 - sel)), icons::CHECK, 13.0, theme::ACCENT);
        }
        let resp = if self.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        ChipResponse { pressed: act && !over_x, removed: act && over_x, response: resp }
    }
}

/// Where items of `widths` go when they wrap in rows `max_w` wide, `gap` apart both ways, rows `row_h` high: each
/// item's top-left offset, and the height they take (0 for none).
pub fn flow(widths: &[f32], max_w: f32, row_h: f32, gap: f32) -> (Vec<Vec2>, f32) {
    let mut out = Vec::with_capacity(widths.len());
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for &w in widths {
        let w = w.min(max_w);
        if x > 0.0 && x + w > max_w {
            x = 0.0;
            y += row_h + gap;
        }
        out.push(vec2(x, y));
        x += w + gap;
    }
    let h = if widths.is_empty() { 0.0 } else { y + row_h };
    (out, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of a page with a button, focused when `focus`: egui's own click, and [`activated_fresh`],
    /// [`Button::confirm`] and [`pressed_fresh`] of three buttons, the focused one's.
    fn pressing(ctx: &egui::Context, events: Vec<egui::Event>, focus: bool) -> (bool, bool, Id) {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() };
        input.events = events;
        let mut seen = (false, false, Id::NULL);
        let mut out = ctx.run_ui(input, |ui| {
            let resp = ui.button("Yes");
            if focus {
                ui.memory_mut(|m| m.request_focus(resp.id));
            }
            seen = (resp.clicked(), activated_fresh(&resp), resp.id);
        });
        out.textures_delta.clear();
        seen
    }

    /// Verify-r21 blocker 2, verify2-r21 m3: egui turns a held Enter's (or Space's) repeats into clicks of the focused
    /// button. What is hard to take back takes a pointer's press, an accessibility tool's, or a fresh key press only; an
    /// accessibility press counts beside an unrelated held key's repeat.
    #[test]
    fn a_fresh_activation_is_no_held_key_s_repeat() {
        use egui::{Event, Key, Modifiers};
        let ctx = egui::Context::default();
        pressing(&ctx, Vec::new(), true);
        let id = pressing(&ctx, Vec::new(), false).2;
        let key = |key: Key, pressed: bool| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
        let tool = || {
            Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: id.accesskit_id(),
                data: None,
            })
        };
        for k in [Key::Enter, Key::Space] {
            let at = |events: Vec<Event>| { let (clicked, fresh, _) = pressing(&ctx, events, false); (clicked, fresh) };
            assert_eq!(at(vec![key(k, true)]), (true, true), "{k:?}: a fresh press");
            assert_eq!(at(vec![key(k, true)]), (true, false), "{k:?} held: egui's click of its repeat is nobody's press");
            assert_eq!(at(vec![key(k, true), tool()]), (true, true), "{k:?} held, and an accessibility tool's press");
            assert_eq!(at(vec![key(k, true)]), (true, false));
            at(vec![key(k, false)]);
            assert_eq!(at(vec![tool()]), (true, true), "an accessibility tool's press");
        }
        // A pointer's, even with a key held.
        pressing(&ctx, vec![key(Key::Enter, true)], false);
        let p = pos2(20.0, 18.0);
        let button = |pressed: bool| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        pressing(&ctx, vec![Event::PointerMoved(p), button(true)], false);
        let (clicked, fresh, _) = pressing(&ctx, vec![key(Key::Enter, true), button(false)], false);
        assert_eq!((clicked, fresh), (true, true), "a pointer's press beside a held key's repeat");
    }

    #[test]
    fn chips_wrap_in_rows() {
        let (at, h) = flow(&[100.0, 120.0, 90.0, 300.0], 260.0, 32.0, 8.0);
        assert_eq!(at, vec![vec2(0.0, 0.0), vec2(108.0, 0.0), vec2(0.0, 40.0), vec2(0.0, 80.0)]);
        assert_eq!(h, 112.0);
        // Wider than a row: alone on its own, cut to the row.
        let (at, h) = flow(&[500.0, 50.0], 260.0, 32.0, 8.0);
        assert_eq!(at, vec![vec2(0.0, 0.0), vec2(0.0, 40.0)]);
        assert_eq!(h, 72.0);
        assert_eq!(flow(&[], 260.0, 32.0, 8.0).1, 0.0);
    }

    #[test]
    fn names_keep_their_colour() {
        assert_eq!(name_color("Alex"), name_color("Alex"));
        let colours: std::collections::HashSet<_> = ["Alex", "Mika", "Robin", "Sora", "Taylor", "Kasumi"].iter().map(|n| name_color(n).to_array()).collect();
        assert!(colours.len() >= 3, "{colours:?}");
    }
}
