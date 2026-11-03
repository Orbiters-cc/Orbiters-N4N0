//! Lists that open from a control: a dropdown (a value picked among options, the version of a package) and a menu of
//! actions (a "…" button). The list grows out of its control with a spring, takes the pointer on press like the other
//! controls, and closes on a press elsewhere, on Escape or once something is picked; while it closes it shrinks back
//! into its control, which bounces as it lands ([`crate::morph`]). A context menu ([`context_menu`]) is the same list,
//! opened by a right press on a card at the pointer.

use crate::anim::{self, Spring};
use crate::theme::{self, weight, Txt};
use crate::widgets::{self, icon, note_control, text, text_truncated};
use crate::{icons, morph};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Order, Rect, Sense, Ui};

/// One row of a list.
#[derive(Clone, Copy, Debug)]
pub struct Item<'a> {
    pub label: &'a str,
    /// A quieter second label on the right (a version's channel, a shortcut).
    pub detail: Option<&'a str>,
    pub glyph: Option<&'a str>,
    /// Red, for destructive actions.
    pub danger: bool,
}

impl<'a> Item<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { label, detail: None, glyph: None, danger: false }
    }

    pub fn detail(mut self, d: &'a str) -> Self {
        self.detail = Some(d);
        self
    }

    pub fn icon(mut self, glyph: &'a str) -> Self {
        self.glyph = Some(glyph);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

const ROW_H: f32 = 34.0;
const PAD: f32 = 6.0;
const MAX_LIST_H: f32 = 300.0;

fn open_id(id: Id) -> Id {
    id.with("popup-open")
}

/// Whether the list of the control `id` is open.
pub fn is_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<bool>(open_id(id))).unwrap_or(false)
}

fn set_open(ctx: &egui::Context, id: Id, open: bool) {
    ctx.data_mut(|d| d.insert_temp(open_id(id), open));
}

/// Whether any list opened from a control is open (a dialog leaves Escape to it).
pub fn any_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(Id::unique("orbiters-popup-open-pass"))).is_some_and(|p| p + 1 >= ctx.cumulative_pass_nr())
}

/// Where a popover (a panel under the button that opened it) is this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Popover {
    /// Its rect now.
    pub rect: Rect,
    /// How opaque it is.
    pub opacity: f32,
    /// Closed and landed back on its button: drop it.
    pub gone: bool,
}

/// A popover sitting at `base` while `open`: it grows out of the corner nearest its `anchor` (the button that opened
/// it) on a bouncy spring and, once closed, shrinks back into it the same way, fading, and the button bounces as it
/// lands ([`morph`]). Keep it (drawing nothing clickable) until [`Popover::gone`]; opened again before, it turns back
/// from where it is.
pub fn popover(ctx: &egui::Context, id: Id, anchor: Rect, base: Rect, open: bool) -> Popover {
    let travel = morph::travel(ctx, id.with("popover"), open, Spring::BOUNCY);
    if travel.landed {
        morph::land(ctx, anchor, travel.landing);
    }
    let k = travel.t;
    let corner = pos2(
        if (base.right() - anchor.center().x).abs() < (base.left() - anchor.center().x).abs() { base.right() } else { base.left() },
        if base.top() >= anchor.center().y { base.top() } else { base.bottom() },
    );
    let s = 0.94 + 0.06 * k;
    let towards = if corner.y == base.top() { -8.0 } else { 8.0 };
    let rect = Rect::from_min_max(corner + (base.min - corner) * s, corner + (base.max - corner) * s).translate(vec2(0.0, towards * (1.0 - k)));
    Popover { rect, opacity: k.clamp(0.0, 1.0), gone: travel.done }
}

/// A dropdown in `rect`: the chosen value and a caret on a neutral pill; pressed, the options open under it, the
/// chosen one ticked. Returns the index picked this frame.
pub fn dropdown(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    rect: Rect,
    value: &str,
    items: &[Item<'_>],
    selected: Option<usize>,
    enabled: bool,
) -> Option<usize> {
    let id = ui.scope_id().with(("dropdown", id_salt));
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    note_control(ui.ctx(), value, rect, enabled);
    let open = is_open(ui.ctx(), id);
    if enabled && widgets::activated(&resp) {
        set_open(ui.ctx(), id, !open);
    }
    let ctx = ui.ctx().clone();
    let hover = anim::toggle(&ctx, id.with("hover"), enabled && (resp.hovered() || open), Spring::SNAPPY);
    let press = anim::toggle(&ctx, id.with("press"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let flip = anim::toggle(&ctx, id.with("caret"), is_open(&ctx, id), Spring::BOUNCY);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.04 * press) * morph::bounce(&ctx, rect));
    let mut p = ui.painter().clone();
    if !enabled {
        p.multiply_opacity(0.45);
    }
    p.rect_filled(r, theme::radius(r.height() * 0.5), theme::mix(theme::control(), theme::INK_600, hover));
    text_truncated(&p, pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(value, 12.5).weight(weight::MEDIUM), r.width() - 14.0 - 30.0);
    // The caret turns over while the list is open.
    let c = pos2(r.right() - 16.0, r.center().y);
    let k = flip.clamp(-0.2, 1.2);
    let s = 3.6;
    let dy = s * 0.5 * (1.0 - 2.0 * k);
    p.line(vec![pos2(c.x - s, c.y - dy), pos2(c.x, c.y + dy), pos2(c.x + s, c.y - dy)], egui::Stroke::new(1.6, theme::FG_2));
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    list(&ctx, id, rect, items, selected, rect.width().max(160.0), false)
}

/// A round "…" (or other `glyph`) button in `rect` with a menu of `items`; returns the index picked this frame.
pub fn menu_button(ui: &mut Ui, id_salt: impl egui::AsIdSalt, rect: Rect, glyph: &str, label: &str, items: &[Item<'_>]) -> Option<usize> {
    let id = ui.scope_id().with(("menu", id_salt));
    let resp = ui.interact(rect, id, Sense::click());
    note_control(ui.ctx(), label, rect, true);
    let open = is_open(ui.ctx(), id);
    if widgets::activated(&resp) {
        set_open(ui.ctx(), id, !open);
    }
    let ctx = ui.ctx().clone();
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() || open, Spring::SNAPPY);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.1 * press) * morph::bounce(&ctx, rect));
    let p = ui.painter();
    p.circle_filled(r.center(), r.width() * 0.5, theme::mix(theme::control(), theme::INK_600, hover));
    icon(p, r.center(), glyph, 17.0, theme::mix(theme::FG_2, theme::FG, hover));
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label);
    list(&ctx, id, rect, items, None, menu_width(ui, items), false)
}

/// The list of `items` for a control its caller draws (a chip): `pressed` this frame opens or closes it, and it closes
/// as the others do (a press elsewhere, Escape, a pick), under `anchor` or above it when there is no room below.
/// Returns the index picked this frame.
pub fn list_from(ctx: &egui::Context, id: Id, anchor: Rect, pressed: bool, items: &[Item<'_>], selected: Option<usize>, width: f32) -> Option<usize> {
    if pressed {
        set_open(ctx, id, !is_open(ctx, id));
    }
    list(ctx, id, anchor, items, selected, width, false)
}

fn menu_width(ui: &Ui, items: &[Item<'_>]) -> f32 {
    let label = items.iter().map(|i| widgets::text_size(ui, &Txt::new(i.label, 13.0)).x).fold(0.0, f32::max);
    let detail = items.iter().filter_map(|i| i.detail).map(|d| widgets::text_size(ui, &Txt::new(d, 11.5)).x + 10.0).fold(0.0, f32::max);
    (label + detail + 2.0 * PAD + 24.0 + 30.0).max(180.0)
}

/// A context menu of `items` for `response` (a card, a row): a right press on it opens the list at the pointer, which
/// closes like the other lists. Returns the index picked this frame. `id_salt` tells apart several menus of one
/// response; the items may change while it is open (they are read every frame), an empty list never opens.
pub fn context_menu(ui: &Ui, response: &egui::Response, id_salt: impl std::hash::Hash + std::fmt::Debug, items: &[Item<'_>]) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let id = response.id.with(("context-menu", id_salt));
    let at_id = id.with("at");
    let secondary = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
    if secondary && !items.is_empty() && response.contains_pointer() {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            ctx.data_mut(|d| d.insert_temp(at_id, p));
            set_open(&ctx, id, true);
        }
    }
    // Kept once opened: the list shrinks back into that point after closing.
    let at = ctx.data(|d| d.get_temp::<egui::Pos2>(at_id))?;
    list(&ctx, id, Rect::from_center_size(at, vec2(2.0, 2.0)), items, None, menu_width(ui, items), true)
}

/// The list of the control `id` under (or over, near the window's bottom) `anchor`, while it is open or closing. `at_point`:
/// a context menu, from the pointer's corner rather than the control's edge.
fn list(ctx: &egui::Context, id: Id, anchor: Rect, items: &[Item<'_>], selected: Option<usize>, width: f32, at_point: bool) -> Option<usize> {
    let open = is_open(ctx, id);
    let travel = morph::travel(ctx, id.with("list"), open, Spring::BOUNCY);
    if travel.landed {
        morph::land(ctx, anchor, travel.landing);
    }
    let k = travel.t;
    if !open && k < 0.02 {
        return None;
    }
    if open {
        // The pass is read first: the context cannot be read while its memory is locked for writing.
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(Id::unique("orbiters-popup-open-pass"), pass));
    }
    let screen = ctx.content_rect();
    let h = (items.len() as f32 * ROW_H + 2.0 * PAD).min(MAX_LIST_H);
    let gap = if at_point { 2.0 } else { 6.0 };
    let below = anchor.bottom() + gap + h <= screen.bottom() - 8.0 || anchor.top() - gap - h < screen.top() + 8.0;
    // Under the control's left edge, or its right edge on the right half of the window (it stays over what it belongs
    // to); a context menu to the right of the pointer unless it would not fit there.
    let left = if at_point {
        if anchor.left() + width <= screen.right() - 8.0 { anchor.left() } else { anchor.right() - width }
    } else if anchor.center().x > screen.center().x {
        anchor.right() - width
    } else {
        anchor.left()
    };
    let left = left.min(screen.right() - 8.0 - width).max(screen.left() + 8.0);
    let base = if below {
        Rect::from_min_size(pos2(left, anchor.bottom() + gap), vec2(width, h))
    } else {
        Rect::from_min_size(pos2(left, anchor.top() - gap - h), vec2(width, h))
    };
    // Grows from the edge that touches the control.
    let kk = k.clamp(0.0, 1.15);
    let grow = 0.92 + 0.08 * kk;
    let size = vec2(base.width() * (0.97 + 0.03 * kk), base.height() * grow);
    let rect = if below {
        Rect::from_min_size(pos2(base.center().x - size.x * 0.5, base.top() - 6.0 * (1.0 - kk)), size)
    } else {
        Rect::from_min_size(pos2(base.center().x - size.x * 0.5, base.bottom() - size.y + 6.0 * (1.0 - kk)), size)
    };
    let mut picked = None;
    let area = egui::Area::new(id.with("list")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        ui.multiply_opacity(k.clamp(0.0, 1.0));
        widgets::panel(ui.painter(), rect, 14.0);
        let inner = rect.shrink(PAD);
        let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("rows"));
        lu.set_clip_rect(inner.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical().id_salt("popup-rows").auto_shrink([false, false]).max_height(inner.height()).show(&mut lu, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for (i, item) in items.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(inner.width(), ROW_H), Sense::hover());
                let resp = ui.interact(r, id.with(("row", i)), if open { Sense::click() } else { Sense::hover() });
                if open && widgets::pressed(&resp) {
                    picked = Some(i);
                }
                let hover = anim::toggle(ctx, id.with(("row-hover", i)), open && resp.hovered(), Spring::SNAPPY);
                let p = ui.painter();
                if hover > 0.01 {
                    let fill = if item.danger { theme::alpha(theme::DANGER, 0.12 * hover) } else { theme::white(0.07 * hover) };
                    p.rect_filled(r, theme::radius(9.0), fill);
                }
                let chosen = selected == Some(i);
                let fg = if item.danger {
                    theme::DANGER
                } else if chosen {
                    theme::FG
                } else {
                    theme::mix(theme::FG_2, theme::FG, hover)
                };
                let mut x = r.left() + 10.0;
                if let Some(g) = item.glyph {
                    icon(p, pos2(x + 8.0, r.center().y), g, 15.0, fg);
                    x += 26.0;
                }
                let right = r.right() - 10.0 - if selected.is_some() { 22.0 } else { 0.0 };
                let detail_w = item.detail.map(|d| widgets::text_size(ui, &Txt::new(d, 11.5)).x + 10.0).unwrap_or(0.0);
                let weight = if chosen { weight::SEMIBOLD } else { weight::REGULAR };
                text_truncated(p, pos2(x, r.center().y), Align2::LEFT_CENTER, &Txt::new(item.label, 13.0).weight(weight).color(fg), right - x - detail_w);
                if let Some(d) = item.detail {
                    text(p, pos2(right, r.center().y), Align2::RIGHT_CENTER, &Txt::new(d, 11.5).color(theme::FG_3));
                }
                if chosen {
                    icon(p, pos2(r.right() - 18.0, r.center().y), icons::CHECK, 14.0, theme::ACCENT);
                }
                if open {
                    resp.on_hover_cursor(CursorIcon::PointingHand);
                }
            }
        });
    });
    if open {
        // A press anywhere else (the control itself toggles on its own) or Escape closes it.
        let pressed_outside =
            ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p)));
        if picked.is_some() || pressed_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            set_open(ctx, id, false);
        }
    }
    picked
}
