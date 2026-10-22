//! Toasts: short messages that spring in at the bottom right, count down with a thin gradient bar, and leave
//! on their own (errors stay longer). Clicking one dismisses it.

use crate::anim::{self, Spring};
use crate::theme::{self, Txt};
use crate::{icons, widgets};
use egui::{pos2, vec2, Context, CursorIcon, Id, Order, Rect, Sense};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

impl Kind {
    fn icon(self) -> (&'static str, egui::Color32) {
        match self {
            Kind::Info => (icons::INFO, theme::CYAN),
            Kind::Success => (icons::CHECK_CIRCLE, theme::OK),
            Kind::Warning => (icons::WARNING, theme::WARN),
            Kind::Error => (icons::X_CIRCLE, theme::DANGER),
        }
    }

    fn lifetime(self) -> f64 {
        if self == Kind::Error {
            7.0
        } else {
            4.2
        }
    }
}

#[derive(Clone, Debug)]
struct Toast {
    id: u64,
    kind: Kind,
    message: String,
    born: f64,
    leaving: Option<f64>,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

const WIDTH: f32 = 380.0;
const LEAVE: f64 = 0.18;

impl Toasts {
    pub fn push(&mut self, ctx: &Context, kind: Kind, message: impl Into<String>) {
        self.next += 1;
        self.items.push(Toast { id: self.next, kind, message: message.into(), born: anim::time(ctx), leaving: None });
        // Keep the four newest, the others leave.
        let alive = self.items.iter().filter(|t| t.leaving.is_none()).count();
        if alive > 4 {
            let now = anim::time(ctx);
            for t in self.items.iter_mut().filter(|t| t.leaving.is_none()).take(alive - 4) {
                t.leaving = Some(now);
            }
        }
        ctx.request_repaint();
    }

    pub fn show(&mut self, ctx: &Context) {
        let now = anim::time(ctx);
        for t in &mut self.items {
            if t.leaving.is_none() && now - t.born >= t.kind.lifetime() {
                t.leaving = Some(now);
            }
        }
        self.items.retain(|t| t.leaving.is_none_or(|l| now - l < LEAVE));
        if self.items.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let mut dismissed = Vec::new();
        egui::Area::new(Id::unique("orbiters-toasts")).order(Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
            let mut bottom = screen.bottom() - 20.0;
            for t in self.items.iter().rev() {
                let (glyph, color) = t.kind.icon();
                let text = Txt::new(&t.message, 13.0).color(theme::FG).line_height(20.0);
                let galley = ui.painter().layout_job(text.job(WIDTH - 16.0 - 17.0 - 12.0 - 16.0));
                let h = galley.size().y + 28.0;
                let id = Id::unique(("orbiters-toast", t.id));
                // Layout: each toast springs to its slot, new ones slide in from the right.
                let y = anim::spring_from(ctx, id.with("y"), [bottom - h], [bottom - h], Spring::SOFT)[0];
                let enter = anim::spring_from(ctx, id.with("enter"), [0.0], [1.0], Spring::BOUNCY)[0];
                let leave = t.leaving.map(|l| ((now - l) / LEAVE).clamp(0.0, 1.0) as f32).unwrap_or(0.0);
                if let Some(l) = t.leaving {
                    anim::since(ctx, l, LEAVE);
                }
                let alpha = (enter.clamp(0.0, 1.0) * (1.0 - leave)).clamp(0.0, 1.0);
                let dx = 60.0 * (1.0 - enter) + 40.0 * leave;
                let scale = 0.92 + 0.08 * enter.min(1.0) - 0.1 * leave;
                let base = Rect::from_min_size(pos2(screen.right() - 20.0 - WIDTH, y), vec2(WIDTH, h));
                let rect = Rect::from_center_size(base.center() + vec2(dx, 0.0), base.size() * scale);
                let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    dismissed.push(t.id);
                }
                let mut p = ui.painter().clone();
                p.multiply_opacity(alpha);
                widgets::panel(&p, rect, 16.0);
                widgets::icon(&p, pos2(rect.left() + 16.0 + 8.5, rect.top() + 14.0 + 10.0), glyph, 17.0, color);
                p.galley(pos2(rect.left() + 16.0 + 17.0 + 12.0, rect.top() + 14.0), galley, theme::FG);
                // Countdown bar: a 2-point line shrinking over seconds needs no more than 30 frames a second.
                let life = t.kind.lifetime();
                let k = 1.0 - (anim::since_every(ctx, t.born, life, 30.0) / life).clamp(0.0, 1.0) as f32;
                if k > 0.0 {
                    // Inside the rounded corners, shrinking towards the left.
                    let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 3.0), pos2(rect.right() - 14.0, rect.bottom() - 1.0));
                    let bar = Rect::from_min_size(track.min, vec2(track.width() * k, track.height()));
                    p.rect_filled(bar, theme::radius(1.0), theme::white(0.35));
                }
                bottom = y - 10.0;
            }
        });
        if !dismissed.is_empty() {
            for t in &mut self.items {
                if dismissed.contains(&t.id) && t.leaving.is_none() {
                    t.leaving = Some(now);
                }
            }
            ctx.request_repaint();
        }
    }
}
