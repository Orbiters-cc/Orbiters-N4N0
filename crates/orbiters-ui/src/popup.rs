//! Lists that open from a control: a dropdown (a value picked among options, the version of a package) and a menu of
//! actions (a "…" button). The list grows out of its control with a spring, takes the pointer on press like the other
//! controls, and closes on a press elsewhere, on Escape or once something is picked; while it closes it shrinks back.

use crate::anim::{self, Spring};
use crate::icons;
use crate::theme::{self, weight, Txt};
use crate::widgets::{self, icon, note_control, text, text_truncated};
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
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.04 * press));
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
    list(&ctx, id, rect, items, selected, rect.width().max(160.0))
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
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.1 * press));
    let p = ui.painter();
    p.circle_filled(r.center(), r.width() * 0.5, theme::mix(theme::control(), theme::INK_600, hover));
    icon(p, r.center(), glyph, 17.0, theme::mix(theme::FG_2, theme::FG, hover));
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label);
    let width = items.iter().map(|i| widgets::text_size(ui, &Txt::new(i.label, 13.0)).x).fold(0.0, f32::max) + 2.0 * PAD + 24.0 + 30.0;
    list(&ctx, id, rect, items, None, width.max(180.0))
}

/// The list of the control `id` under (or over, near the window's bottom) `anchor`, while it is open or closing.
fn list(ctx: &egui::Context, id: Id, anchor: Rect, items: &[Item<'_>], selected: Option<usize>, width: f32) -> Option<usize> {
    let open = is_open(ctx, id);
    let k = anim::spring_from(ctx, id.with("list-k"), [0.0], [if open { 1.0 } else { 0.0 }], Spring::BOUNCY)[0];
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
    let below = anchor.bottom() + 6.0 + h <= screen.bottom() - 8.0 || anchor.top() - 6.0 - h < screen.top() + 8.0;
    // Under the control's left edge, or its right edge on the right half of the window (it stays over what it belongs to).
    let left = if anchor.center().x > screen.center().x { anchor.right() - width } else { anchor.left() };
    let left = left.min(screen.right() - 8.0 - width).max(screen.left() + 8.0);
    let base = if below {
        Rect::from_min_size(pos2(left, anchor.bottom() + 6.0), vec2(width, h))
    } else {
        Rect::from_min_size(pos2(left, anchor.top() - 6.0 - h), vec2(width, h))
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
