//! A page's static header: a caption, the title and a line under it on the left and, on the same row on the right, a
//! toolbar (a search field and the page's actions). On windows too narrow for both, the secondary actions become round
//! icon buttons, then the toolbar takes a row of its own under the title. The content scrolls on under the header,
//! blurred ([`super::under_top`] down to [`Placed::band_bottom`]), its scroll bar starting at [`Placed::content_top`].
//!
//! Lay it out with [`Header::place`] before the content (which starts at `content_top`), then draw it with
//! [`Header::show`] after the content and [`super::under_top`], so that it paints and takes clicks over what scrolled
//! under it.

use super::{Cx, PAD_X};
use egui::{pos2, vec2, Align2, Rect, Ui, UiBuilder};
use orbiters_ui::feedback;
use orbiters_ui::layout::{self, Fit};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Variant};

/// One action of the toolbar.
pub struct Action<'a> {
    /// What tells it apart (its widget id, and the key [`Header::show`] returns when it is pressed).
    pub key: &'static str,
    pub label: &'a str,
    pub glyph: &'a str,
    /// Green, labelled even in the compact toolbar.
    pub primary: bool,
    /// Always a round icon button (its label as the tooltip).
    pub icon_only: bool,
    pub enabled: bool,
    /// A spinner instead of its icon (a refresh under way).
    pub spinning: bool,
}

impl<'a> Action<'a> {
    pub fn new(key: &'static str, label: &'a str, glyph: &'a str) -> Self {
        Self { key, label, glyph, primary: false, icon_only: false, enabled: true, spinning: false }
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn icon_only(mut self) -> Self {
        self.icon_only = true;
        self
    }

    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn spinning(mut self, s: bool) -> Self {
        self.spinning = s;
        self
    }
}

/// A page's header.
pub struct Header<'a> {
    pub caption: &'a str,
    pub title: &'a str,
    pub sub: &'a str,
    /// A spinner before the line under the title (the page is still finding things).
    pub busy: bool,
    /// A search field first in the toolbar: its id salt and its hint.
    pub search: Option<(&'static str, &'a str)>,
    pub actions: Vec<Action<'a>>,
}

/// Where a header's parts go.
#[derive(Clone, Debug)]
pub struct Placed {
    /// The caption, title and line under it.
    pub head: Rect,
    pub fit: Fit,
    pub search: Option<Rect>,
    /// Each action's rect, in the order given.
    pub actions: Vec<Rect>,
    /// Where the content rests (and the scroll bar starts).
    pub content_top: f32,
    /// The bottom of the band where content passing under the header is blurred.
    pub band_bottom: f32,
}

const ROW_H: f32 = 40.0;
const GAP: f32 = 8.0;
const SEARCH_W: f32 = 240.0;
const SEARCH_COMPACT_W: f32 = 200.0;

impl Header<'_> {
    fn title_width(&self, ui: &Ui) -> f32 {
        let title = widgets::text_size(ui, &Txt::new(self.title, 30.0).weight(weight::SEMIBOLD).tracking(-0.015)).x;
        let sub = widgets::text_size(ui, &Txt::new(self.sub, 13.0)).x + if self.busy { 22.0 } else { 0.0 };
        title.max(sub.min(300.0))
    }

    fn widths(&self, ui: &Ui, compact: bool) -> Vec<f32> {
        self.actions.iter().map(|a| if a.icon_only || (compact && !a.primary) { ROW_H } else { Button::new(a.label).icon(a.glyph).desired_width(ui) }).collect()
    }

    /// Lays the header out on `page` (the page's rect, from the window's top).
    pub fn place(&self, ui: &Ui, page: Rect, cx: &Cx) -> Placed {
        let head = Rect::from_min_size(pos2(page.left() + PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - PAD_X * 2.0, 72.0));
        let full_w = self.widths(ui, false);
        let compact_w = self.widths(ui, true);
        let total = |ws: &[f32], search: f32| ws.iter().sum::<f32>() + GAP * ws.len() as f32 + search;
        let has_search = self.search.is_some();
        let fit = layout::toolbar(
            head.width(),
            self.title_width(ui),
            total(&full_w, if has_search { SEARCH_W } else { -GAP }),
            total(&compact_w, if has_search { SEARCH_COMPACT_W } else { -GAP }),
        );
        let (widths, row, search_w) = match fit {
            Fit::Beside => (full_w, Rect::from_min_max(pos2(head.left(), head.bottom() - ROW_H), head.max), SEARCH_W),
            Fit::Compact => (compact_w, Rect::from_min_max(pos2(head.left(), head.bottom() - ROW_H), head.max), SEARCH_COMPACT_W),
            Fit::Below => {
                let row = Rect::from_min_size(pos2(head.left(), head.bottom() + 12.0), vec2(head.width(), ROW_H));
                let rest = (row.width() - total(&full_w, 0.0)).clamp(160.0, 420.0);
                (full_w, row, rest)
            }
        };
        // Right to left: the last action at the row's right edge.
        let mut x = row.right();
        let mut actions = vec![Rect::NOTHING; widths.len()];
        for (i, w) in widths.iter().enumerate().rev() {
            actions[i] = Rect::from_min_size(pos2(x - w, row.top()), vec2(*w, ROW_H));
            x -= w + GAP;
        }
        let search = has_search.then(|| match fit {
            Fit::Below => Rect::from_min_size(row.min, vec2(search_w, ROW_H)),
            _ => Rect::from_min_size(pos2(x - search_w, row.top()), vec2(search_w, ROW_H)),
        });
        let bottom = if fit == Fit::Below { row.bottom() } else { head.bottom() };
        Placed { head, fit, search, actions, content_top: bottom + 20.0, band_bottom: bottom + 8.0 }
    }

    /// Draws the header as placed, `query` in its search field; returns the key of the action pressed.
    pub fn show(&self, ui: &mut Ui, cx: &Cx, placed: &Placed, query: Option<&mut String>) -> Option<&'static str> {
        let (head, alpha) = super::entering(ui, cx, 0, placed.head);
        let p = ui.painter().clone();
        let mut hp = p.clone();
        hp.multiply_opacity(alpha);
        widgets::text(&hp, pos2(head.left(), head.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps(self.caption));
        widgets::text(&hp, pos2(head.left(), head.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(self.title, 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
        // The line under the title stops short of the toolbar.
        let toolbar_left = match placed.fit {
            Fit::Below => head.right(),
            _ => placed.search.iter().chain(placed.actions.iter()).map(|r| r.left()).fold(head.right(), f32::min) - layout::TOOLBAR_GAP,
        };
        let mut x = head.left();
        if self.busy {
            feedback::spinner(&hp, pos2(x + 7.0, head.top() + 62.0), 6.0, 1.8, theme::ACCENT);
            x += 22.0;
        }
        widgets::sliding_value(
            ui,
            ui.scope_id().with(("header-sub", self.title)),
            pos2(x, head.top() + 62.0),
            Align2::LEFT_CENTER,
            self.sub,
            &Txt::new("", 13.0).color(theme::alpha(theme::FG_3, alpha)),
            (toolbar_left - x).max(40.0),
        );

        let (_, bar_alpha) = super::entering(ui, cx, 1, placed.head);
        let mut tu = ui.new_child(UiBuilder::new().max_rect(ui.max_rect()).id_salt(("header-toolbar", self.title)));
        tu.multiply_opacity(bar_alpha);
        if let (Some((salt, hint)), Some(r), Some(q)) = (self.search, placed.search, query) {
            let mut su = tu.new_child(UiBuilder::new().max_rect(r).id_salt(salt));
            widgets::search_field(&mut su, salt, q, hint, r.width());
        }
        let mut pressed = None;
        for (a, r) in self.actions.iter().zip(&placed.actions) {
            let round = (r.width() - ROW_H).abs() < 0.5 && (a.icon_only || !a.primary);
            let resp = if round {
                let b = IconButton::new(if a.spinning { "" } else { a.glyph }, a.label).size(ROW_H, 17.0).backdrop(theme::control()).id_salt(a.key);
                let resp = b.show_at(&mut tu, *r);
                if a.spinning {
                    feedback::spinner(tu.painter(), r.center(), 7.5, 2.0, theme::FG_2);
                }
                resp
            } else {
                Button::new(a.label)
                    .icon(a.glyph)
                    .variant(if a.primary { Variant::Primary } else { Variant::Secondary })
                    .enabled(a.enabled)
                    .width(r.width())
                    .id_salt(a.key)
                    .show_at(&mut tu, *r)
            };
            if a.enabled && widgets::activated(&resp) {
                pressed = Some(a.key);
            }
        }
        pressed
    }
}
