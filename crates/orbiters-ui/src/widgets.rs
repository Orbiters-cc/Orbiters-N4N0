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

/// Notes where a control labelled `label` was drawn, for [`control`] (an automated run only).
pub fn note_control(ctx: &egui::Context, label: &str, rect: Rect, enabled: bool) {
    if !WATCH.load(Ordering::Relaxed) {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Drawn>(drawn_id()).0.insert(label.to_string(), (rect, enabled, pass));
    });
}

/// Paints a Phosphor glyph centred on `center` (or N4N0's face, [`icons::N4N0`]).
pub fn icon(painter: &Painter, center: Pos2, glyph: &str, size: f32, color: Color32) {
    if glyph == icons::N4N0 {
        return crate::n4n0_face::icon(painter, center, size, color);
    }
    painter.text(center, Align2::CENTER_CENTER, glyph, FontId::new(size, fonts::icons()), color);
}

/// Paints the filled variant of a Phosphor glyph (N4N0's face is filled already).
pub fn icon_fill(painter: &Painter, center: Pos2, glyph: &str, size: f32, color: Color32) {
    if glyph == icons::N4N0 {
        return crate::n4n0_face::icon(painter, center, size, color);
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
        self.paint(ui, rect, &resp, size, gap, icon_size);
        let resp = if self.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        match self.hint {
            Some(hint) => resp.on_hover_text(hint),
            None => resp,
        }
    }

    /// Shows the button in a given rect (for layouts that place it themselves). Its id comes from its label and icon
    /// (or [`Button::id_salt`]), never from where it is: a button that moves (scrolling, an entrance, a scale-in)
    /// stays one widget, so a press on it still clicks and no state is left behind at every position it passed.
    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let (_, _, size, gap, icon_size) = self.metrics();
        let key = self.id_salt.unwrap_or_else(|| IdSalt::new((self.label, self.icon)));
        let resp = ui.interact(rect, ui.scope_id().with("button").with_salt(key), if self.enabled { Sense::click() } else { Sense::hover() });
        if self.enabled {
            pressed(&resp);
        }
        note_control(ui.ctx(), self.label, rect, self.enabled);
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
        let scale = 1.0 - 0.04 * press;
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
            Variant::Glass => {
                painter.rect_filled(bg, theme::radius(r), theme::mix(theme::surface_strong(), theme::INK_700, hover));
                theme::FG
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

/// Round icon button (36 points), lighter on hover, deeper press.
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
        self.paint(ui, rect, &resp);
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(self.label)
    }

    /// Shows the button in a given rect; its id comes from its icon and label (or [`IconButton::id_salt`]), never
    /// from where it is.
    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let key = self.id_salt.unwrap_or_else(|| IdSalt::new((self.glyph, self.label)));
        let resp = ui.interact(rect, ui.scope_id().with("icon-button").with_salt(key), Sense::click());
        pressed(&resp);
        note_control(ui.ctx(), self.label, rect, true);
        self.paint(ui, rect, &resp);
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(self.label)
    }

    fn paint(&self, ui: &Ui, rect: Rect, resp: &Response) {
        let ctx = ui.ctx();
        let hover = anim::toggle(ctx, resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let press = anim::toggle(ctx, resp.id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.12 * press));
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

/// Segmented control whose highlight slides between options with a bouncy spring. Returns true when the value
/// changed (on press).
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut T, options: &[SegOption<'_, T>], small: bool) -> bool {
    let id = ui.scope_id().with(id_salt);
    let (item_h, pad_x, size) = if small { (28.0, 12.0, 12.0) } else { (32.0, 14.0, 13.0) };
    let badge_txt = |b: &str| Txt::new(b.to_uppercase(), 9.5).weight(weight::SEMIBOLD).tracking(0.04).color(theme::FG_2);
    let widths: Vec<f32> = options
        .iter()
        .map(|o| {
            let t = text_size(ui, &Txt::new(o.label, size).weight(weight::MEDIUM)).x;
            let b = o.badge.map(|b| text_size(ui, &badge_txt(b)).x + 12.0 + 6.0).unwrap_or(0.0);
            t + b + pad_x * 2.0
        })
        .collect();
    let track_w = widths.iter().sum::<f32>() + 8.0;
    let (track, _) = ui.allocate_exact_size(vec2(track_w, item_h + 8.0), Sense::hover());
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
        let x0 = rects[i].center().x - (lw + bw) * 0.5;
        text(&p, pos2(x0, rects[i].center().y), Align2::LEFT_CENTER, &label);
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

/// Switch: the knob springs across, the track turns Orbiters green. Acts on press.
pub fn toggle(ui: &mut Ui, id_salt: impl egui::AsIdSalt, on: &mut bool) -> bool {
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 28.0), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let mut changed = false;
    if pressed(&resp) {
        *on = !*on;
        changed = true;
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
    let id = ui.scope_id().with(id_salt);
    let (min, max) = (*range.start(), *range.end());
    let (rect, _) = ui.allocate_exact_size(vec2(192.0 + 12.0 + 80.0, 28.0), Sense::hover());
    let track_rect = Rect::from_min_size(rect.min, vec2(192.0, 28.0));
    let resp = ui.interact(track_rect, id, Sense::click_and_drag());
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
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    field_frame(ui, rect, id, value, hint, Well { mono, leading: None, radius: 12.0 })
}

/// Search field: pill with a magnifier.
pub fn search_field(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, hint: &str, width: f32) -> Response {
    let id = ui.scope_id().with(id_salt);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    field_frame(ui, rect, id, value, hint, Well { mono: false, leading: Some(icons::MAGNIFYING_GLASS), radius: 20.0 })
}

/// Look of a text well.
struct Well<'a> {
    mono: bool,
    leading: Option<&'a str>,
    radius: f32,
}

fn field_frame(ui: &mut Ui, rect: Rect, id: Id, value: &mut String, hint: &str, well: Well<'_>) -> Response {
    let Well { mono, leading, radius } = well;
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
    ui.put(inner, edit)
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
    let p = ui.painter();
    let mut y = rect.center().y - text_h * 0.5;
    p.galley(pos2(rect.left(), y), label_g.clone(), theme::FG);
    y += label_g.size().y + 2.0;
    if let Some(g) = desc_g {
        p.galley(pos2(rect.left(), y), g, theme::FG_3);
    }
    let control_rect = Rect::from_min_max(pos2(rect.right() - control_width, rect.top()), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    control(&mut child);
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
