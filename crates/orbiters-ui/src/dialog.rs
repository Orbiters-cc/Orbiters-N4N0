//! A dialog that grows out of what opened it (a button, a card) into the middle of the window and shrinks back into it
//! when it closes, its height following its content with a spring when that changes (another step of the same flow).
//! The page under it dims and takes no clicks; a click there, Escape or its × closes it. Closing is the opening run
//! backwards: the contents go first, the shape turns back into its origin's colour, fades out over it as it lands and
//! the origin bounces ([`crate::morph`]; buttons from [`crate::widgets`] do on their own).
//!
//! The caller keeps the dialog while [`Response::closed`] is false: [`close`] (or the person) starts the closing
//! morph, and `closed` comes once it landed. [`reopen`] turns a closing dialog back from where it is (its button
//! pressed again); a new `id` (a serial) opens a new dialog.

use crate::anim::{self, Spring};
use crate::theme::{self, weight, Txt};
use crate::widgets;
use crate::{icons, morph, paint, popup};
use egui::{pos2, vec2, Align2, Id, Order, Rect, Sense, Ui, UiBuilder};

/// Which part of the dialog the contents closure is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The scrolling body.
    Body,
    /// The row of buttons at the bottom (right to left), when the dialog has one.
    Footer,
}

/// What became of a dialog this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    /// Closing (the morph back runs).
    pub closing: bool,
    /// Closed: the morph back ended, drop the dialog.
    pub closed: bool,
}

pub struct Dialog<'a> {
    id: Id,
    origin: Rect,
    width: f32,
    glyph: Option<&'a str>,
    title: &'a str,
    subtitle: Option<&'a str>,
    footer: bool,
    busy: bool,
    origin_fill: egui::Color32,
    order: Order,
}

fn closing_id(id: Id) -> Id {
    id.with("dialog-closing")
}

/// Starts the closing morph of the dialog `id`.
pub fn close(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(closing_id(id), true));
}

/// Turns the closing dialog `id` back open from where its morph is.
pub fn reopen(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| d.remove_temp::<bool>(closing_id(id)));
}

/// How much of what the dialog `id` grew out of shows: none while it is out, all of it from the hand-over on (it lands
/// over its origin, which then bounces). A page that hides its button while the dialog is out shows it this much.
pub fn source_alpha(ctx: &egui::Context, id: Id) -> f32 {
    morph::source_alpha_by(ctx.data(|d| d.get_temp::<f32>(id.with("dialog-t"))).unwrap_or(0.0), morph::HANDOVER_LATE)
}

pub fn is_closing(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<bool>(closing_id(id))).unwrap_or(false)
}

/// Space between the dialog and the window's edges.
const MARGIN: f32 = 40.0;
const PAD: f32 = 24.0;
const HEAD_H: f32 = 44.0;
const FOOT_H: f32 = 40.0;
/// Room around the body's content inside its scroll.
const BODY_ROOM: i8 = 6;
/// More of it on the right, reaching into the dialog's padding: egui's bar sits at the scroll area's own right edge, and
/// there it runs beside the content and not over what reaches its edge (a count at the end of a heading, a button).
const BAR_GUTTER: i8 = 10;

impl<'a> Dialog<'a> {
    /// A dialog `id` titled `title` that grows out of `origin`.
    pub fn new(id: Id, origin: Rect, title: &'a str) -> Self {
        Self { id, origin, width: 640.0, glyph: None, title, subtitle: None, footer: false, busy: false, origin_fill: theme::surface_strong(), order: Order::Foreground }
    }

    /// The colour of what it grows out of (a green button, a card), which it turns from into its own.
    pub fn origin_fill(mut self, c: egui::Color32) -> Self {
        self.origin_fill = c;
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }

    /// The tinted icon tile beside the title.
    pub fn glyph(mut self, g: &'a str) -> Self {
        self.glyph = Some(g);
        self
    }

    pub fn subtitle(mut self, s: &'a str) -> Self {
        self.subtitle = Some(s);
        self
    }

    /// A row of buttons under the body ([`Part::Footer`]).
    pub fn footer(mut self, f: bool) -> Self {
        self.footer = f;
        self
    }

    /// Something runs that it shows: a click beside it or Escape does not close it (its × still does).
    pub fn busy(mut self, b: bool) -> Self {
        self.busy = b;
        self
    }

    /// The layer it is drawn in (the foreground, by default): one opened from what is drawn above it (a 3D view shown in
    /// the tooltip layer) goes there too, and is drawn over it as the newer area.
    pub fn order(mut self, order: Order) -> Self {
        self.order = order;
        self
    }

    /// Shows the dialog; `contents` is called for its body, then for its footer.
    pub fn show(self, ctx: &egui::Context, mut contents: impl FnMut(&mut Ui, Part)) -> Response {
        let id = self.id;
        let closing = is_closing(ctx, id);
        let travel = morph::travel(ctx, id, !closing, Spring::MORPH);
        let t = travel.t;
        ctx.data_mut(|d| d.insert_temp(id.with("dialog-t"), t));
        if travel.done {
            ctx.data_mut(|d| d.remove_temp::<bool>(closing_id(id)));
            return Response { closing: true, closed: true };
        }
        if travel.landed {
            morph::land(ctx, self.origin, travel.landing);
        }
        let screen = ctx.content_rect();
        let width = self.width.min(screen.width() - 2.0 * MARGIN).max(280.0);
        let max_h = (screen.height() - 2.0 * MARGIN).max(240.0);
        let foot = if self.footer { 16.0 + FOOT_H } else { 0.0 };
        let chrome = PAD + HEAD_H + 16.0 + foot + PAD;
        // The body as tall as its content (measured last frame), within the window.
        let measured = ctx.data(|d| d.get_temp::<f32>(id.with("content-h"))).unwrap_or(220.0);
        let body_target = measured.min(max_h - chrome).max(40.0);
        let body_h = anim::spring(ctx, id.with("body-h"), [body_target], Spring::SOFT)[0].max(40.0);
        let size = vec2(width, chrome + body_h);
        let fin = Rect::from_center_size(screen.center(), size);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let shape = Rect::from_min_max(
            pos2(lerp(self.origin.left(), fin.left()), lerp(self.origin.top(), fin.top())),
            pos2(lerp(self.origin.right(), fin.right()), lerp(self.origin.bottom(), fin.bottom())),
        );
        let radius = lerp((self.origin.height() * 0.5).min(18.0), 24.0).max(4.0);
        let k = t.clamp(0.0, 1.0);
        // The contents fly with the shape, scaled as one (never unequally) to fit inside it, so that they grow out of
        // what opened the dialog instead of being cut by it; they fade in on the way and leave first when it closes.
        let content = if closing { ((t - 0.55) / 0.35).clamp(0.0, 1.0) } else { ((t - 0.2) / 0.6).clamp(0.0, 1.0) };
        let scale = (shape.width() / fin.width()).min(shape.height() / fin.height()).clamp(0.05, 1.1);
        let flight = egui::emath::TSTransform::new(shape.center().to_vec2() - fin.center().to_vec2() * scale, scale);
        let landed = (scale - 1.0).abs() < 0.002 && (shape.center() - fin.center()).length() < 0.25;
        let mut response = Response { closing, closed: false };

        // Over its origin near it: fading in as it leaves, out as it lands (the origin then bounces), once it is nearly
        // the origin's size.
        let solid = morph::shape_alpha_by(t, morph::HANDOVER_LATE);
        // egui fades new areas in: this one grows out of its origin instead. While it closes, the page under it takes
        // presses again (its button reopens it).
        egui::Area::new(id.with("dialog-area")).order(self.order).fixed_pos(screen.min).fade_in(false).interactable(!closing).show(ctx, |ui| {
            ui.set_min_size(screen.size());
            let scrim = ui.interact(screen, id.with("scrim"), Sense::click());
            ui.painter().rect_filled(screen, 0.0, theme::black(0.55 * morph::smoothstep(0.08, 0.5, t)));
            if !closing && !self.busy && widgets::pressed(&scrim) {
                close(ctx, id);
            }
            let p = ui.painter();
            paint::shadow(p, shape, radius, vec2(0.0, 18.0), 40.0, -10.0, theme::black(0.5 * k * solid));
            // From the colour of what it grew out of to its own, early in the flight.
            widgets::frame(p, shape, radius, theme::mix(self.origin_fill, theme::INK_850, (k * 1.7).clamp(0.0, 1.0)).gamma_multiply(solid));
            let _ = ui.interact(shape, id.with("shape"), Sense::click());

            // The contents are laid out where the dialog lands and drawn through the flight's transform (pictures and
            // text included); clicks wait until it landed, as the transform moves only what is drawn.
            let mut du = ui.new_child(UiBuilder::new().max_rect(fin).id_salt(id.with("contents")));
            du.set_clip_rect(fin.shrink(2.0));
            du.multiply_opacity(content);
            if closing || content < 0.05 {
                du.disable();
            }
            if landed {
                self.contents(&mut du, ctx, fin, body_h, true, &mut contents);
            } else {
                du.with_visual_transform(flight, |du| self.contents(du, ctx, fin, body_h, false, &mut contents));
                let _ = ui.interact(shape.union(fin), id.with("flying"), Sense::click_and_drag());
            }
        });
        if !closing && !self.busy && !popup::any_open(ctx) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close(ctx, id);
        }
        response.closing = is_closing(ctx, id);
        response
    }

    /// The heading, the body and the footer, laid out in `fin` (where the dialog lands); the body's scroll bar only
    /// once `landed`.
    fn contents(&self, du: &mut Ui, ctx: &egui::Context, fin: Rect, body_h: f32, landed: bool, contents: &mut impl FnMut(&mut Ui, Part)) {
        let id = self.id;
        let inner = fin.shrink(PAD);
        // Heading: icon tile, title and subtitle, the ×.
        let head = Rect::from_min_size(inner.min, vec2(inner.width(), HEAD_H));
        let mut x = head.left();
        let p = du.painter();
        if let Some(g) = self.glyph {
            let tile = Rect::from_min_size(pos2(x, head.center().y - 20.0), vec2(40.0, 40.0));
            p.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.12));
            widgets::icon(p, tile.center(), g, 19.0, theme::ACCENT);
            x = tile.right() + 14.0;
        }
        let title_w = head.right() - 48.0 - x;
        match self.subtitle {
            Some(sub) => {
                widgets::text_truncated(p, pos2(x, head.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(self.title, 18.0).weight(weight::SEMIBOLD), title_w);
                widgets::text_truncated(p, pos2(x, head.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3), title_w);
            }
            None => {
                widgets::text_truncated(p, pos2(x, head.center().y), Align2::LEFT_CENTER, &Txt::new(self.title, 18.0).weight(weight::SEMIBOLD), title_w);
            }
        }
        let close_r = Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(32.0, 32.0));
        if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(32.0, 16.0).id_salt(id).show_at(du, close_r)) {
            close(ctx, id);
        }

        // Body, scrolling once it is taller than the window allows.
        let body = Rect::from_min_size(pos2(inner.left(), head.bottom() + 16.0), vec2(inner.width(), body_h));
        // A few points of room around the content (selection rings, a pressed tile) inside the scroll's clip.
        let mut room = body.expand(BODY_ROOM as f32);
        room.max.x += BAR_GUTTER as f32;
        let mut bu = du.new_child(UiBuilder::new().max_rect(room).id_salt(id.with("body")));
        bu.set_clip_rect(room.intersect(du.clip_rect()));
        let bars = if landed { egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded } else { egui::scroll_area::ScrollBarVisibility::AlwaysHidden };
        // A body scrolling itself does it at once and for sure (the results of a search brought into view, a list held still
        // while something appears above it): egui's animated requests are dropped the moment the pointer is over the area,
        // which it is, having just pressed something there.
        let out = egui::ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .auto_shrink([false, false])
            .max_height(room.height())
            .scroll_bar_visibility(bars)
            .animated(false)
            .show(&mut bu, |ui| {
                let room = egui::Margin { left: BODY_ROOM, right: BODY_ROOM + BAR_GUTTER, top: BODY_ROOM, bottom: BODY_ROOM };
                egui::Frame::NONE.inner_margin(room).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 12.0);
                    contents(ui, Part::Body);
                });
            });
        ctx.data_mut(|d| d.insert_temp(id.with("content-h"), (out.content_size.y - 2.0 * BODY_ROOM as f32).ceil()));

        if self.footer {
            let footer = Rect::from_min_size(pos2(inner.left(), body.bottom() + 16.0), vec2(inner.width(), FOOT_H));
            let mut fu = du.new_child(UiBuilder::new().max_rect(footer).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt(id.with("footer")));
            fu.spacing_mut().item_spacing = vec2(8.0, 0.0);
            contents(&mut fu, Part::Footer);
        }
    }
}
