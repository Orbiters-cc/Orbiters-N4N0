//! Photos & captures' selection and dragging out. Ctrl-click, Shift-click, the check on a card's corner or a rubber
//! band select cards (a green ring and check, the badges stepping aside); while some are selected a click adds or
//! removes one, Ctrl+A selects every card shown and Escape lets go. A bar at the bottom says how many, shows them, and
//! offers Select all and Clear.
//!
//! Dragging a card, or any card of the selection, out of the window hands their files to the app it is dropped on
//! ([`crate::drag_out`], as Explorer would): a VRChat picture is its PNG, a 3D capture or recording the picture of its 3D
//! view (`capture_picture`, its thumbnail before it is reconstructed). Items only on Orbiters are not on this computer and
//! are left out. The cards lift (smaller, faded) as the drag starts and settle back once it ended; the bar says what was
//! copied.
use crate::controller::shares::{Entry, Local};
use crate::drag_out::{self, Outcome};
use egui::{pos2, vec2, Align2, Id, Order, Pos2, Rect, Response, Sense, Stroke, Ui};
use orbiters_ui::{
    anim::{self, ease, Spring},
    icons, paint,
    theme::{self, weight, Txt},
    widgets::{self, Button, Size, Variant},
};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// How long the cards lift before Windows' drag starts (its loop is modal: the window shows what it drew last).
const LIFT: f64 = 0.09;
/// How long the bar says what a drag copied.
const SAID_FOR: f64 = 2.6;
/// The check on a card's corner.
pub const CHECK: f32 = 24.0;

#[derive(Default)]
pub struct Selection {
    pub keys: BTreeSet<String>,
    anchor: Option<String>,
    /// Each card drawn this frame and the last (screen rects): the rubber band selects what it crosses.
    rects: Vec<(String, Rect)>,
    last_rects: Vec<(String, Rect)>,
    /// The rubber band: where it started, and what was selected before it (Ctrl adds to it).
    band: Option<(Pos2, BTreeSet<String>)>,
    /// What is being dragged out (drawn lifted) and since when; `started` once Windows has it.
    carried: Vec<String>,
    lifted_at: f64,
    started: bool,
    /// What the last drag copied, and when.
    said: Option<(String, f64)>,
}

/// What a press on a card asked for.
pub enum CardAct {
    None,
    Open,
}

impl Selection {
    pub fn any(&self) -> bool {
        !self.keys.is_empty()
    }
    pub fn has(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
    pub fn carried(&self, key: &str) -> bool {
        self.carried.iter().any(|k| k == key)
    }
    pub fn clear(&mut self) {
        self.keys.clear();
        self.anchor = None;
    }

    /// Before the grid draws: last frame's cards become the band's targets.
    pub fn begin(&mut self) {
        self.last_rects = std::mem::take(&mut self.rects);
    }

    /// A card as drawn this frame.
    pub fn place(&mut self, key: &str, rect: Rect) {
        self.rects.push((key.to_owned(), rect));
    }

    /// A card's response this frame: opens it, changes the selection, or starts dragging it (with the selection when
    /// it is part of it). `order` is every card shown, in order (Shift selects the run between two).
    pub fn card(&mut self, resp: &Response, e: &Entry, check: Rect, order: &[Entry]) -> CardAct {
        let ctx = resp.ctx.clone();
        if resp.drag_started_by(egui::PointerButton::Primary) && self.carried.is_empty() {
            self.lift(&ctx, e, order);
            return CardAct::None;
        }
        if !resp.clicked() {
            return CardAct::None;
        }
        let (m, on_check) = ctx.input(|i| (i.modifiers, i.pointer.interact_pos().is_some_and(|p| check.expand(4.0).contains(p))));
        if m.shift {
            let from = self.anchor.as_ref().and_then(|a| order.iter().position(|o| &o.key == a)).unwrap_or(0);
            let to = order.iter().position(|o| o.key == e.key).unwrap_or(from);
            if !m.command {
                self.keys.clear();
            }
            self.keys.extend(order[from.min(to)..=from.max(to)].iter().map(|o| o.key.clone()));
            return CardAct::None;
        }
        if m.command || on_check || self.any() {
            if !self.keys.remove(&e.key) {
                self.keys.insert(e.key.clone());
            }
            self.anchor = Some(e.key.clone());
            return CardAct::None;
        }
        self.anchor = Some(e.key.clone());
        CardAct::Open
    }

    /// Screenshots: these cards selected.
    pub fn stage(&mut self, keys: Vec<String>) {
        self.keys = keys.into_iter().collect();
    }

    /// Screenshots: these cards lifted as if Windows had their drag, their drag image written to `image`.
    pub fn stage_carry(&mut self, entries: &[Entry], image: Option<&std::path::Path>) {
        self.carried = entries.iter().map(|e| e.key.clone()).collect();
        self.started = true;
        if let Some(path) = image {
            let refs: Vec<&Entry> = entries.iter().collect();
            if let Some(i) = drag_image(&refs) {
                let _ = image::save_buffer(path, &i.rgba, i.width, i.height, image::ExtendedColorType::Rgba8);
            }
        }
    }

    /// Screenshots: the bar says this, as after a drag.
    pub fn stage_said(&mut self, ctx: &egui::Context, text: &str) {
        self.carried.clear();
        self.said = Some((text.into(), anim::time(ctx)));
    }

    /// The drag picks up the card, or the whole selection when the card is in it.
    fn lift(&mut self, ctx: &egui::Context, e: &Entry, order: &[Entry]) {
        let keys: Vec<String> =
            if self.keys.contains(&e.key) { order.iter().filter(|o| self.keys.contains(&o.key)).map(|o| o.key.clone()).collect() } else { vec![e.key.clone()] };
        let entries: Vec<&Entry> = keys.iter().filter_map(|k| order.iter().find(|o| &o.key == k)).collect();
        if !entries.iter().any(|e| e.local.is_some()) {
            self.said = Some(("Only what is on this computer can be dragged out".into(), anim::time(ctx)));
            return;
        }
        self.carried = keys;
        self.lifted_at = anim::time(ctx);
        self.started = false;
    }

    /// Every frame after the grid: keyboard, the rubber band, the lifted drag's start and end. `area` is where cards
    /// are (the band starts on the grid's empty space, `background`).
    pub fn update(&mut self, ui: &Ui, order: &[Entry], background: &Response, modal: bool) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let typing = ctx.memory(|m| m.focused().is_some());
        if !modal && !typing {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
                self.keys = order.iter().map(|e| e.key.clone()).collect();
            }
            if self.any() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.clear();
            }
        }
        // Gone from the page (another filter, a search): no longer selected.
        if self.any() {
            self.keys.retain(|k| order.iter().any(|e| &e.key == k));
        }

        // The rubber band, from the grid's empty space.
        if !modal && background.drag_started_by(egui::PointerButton::Primary) {
            if let Some(at) = ctx.input(|i| i.pointer.press_origin()) {
                let base = if ctx.input(|i| i.modifiers.command || i.modifiers.shift) { self.keys.clone() } else { BTreeSet::new() };
                self.band = Some((at, base));
            }
        }
        if let Some((start, base)) = self.band.clone() {
            let down = ctx.input(|i| i.pointer.primary_down());
            let at = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(start);
            let band = Rect::from_two_pos(start, at);
            self.keys = base;
            for (k, r) in &self.last_rects {
                if r.intersects(band) {
                    self.keys.insert(k.clone());
                }
            }
            let p = ui.ctx().layer_painter(egui::LayerId::new(Order::Middle, Id::unique("gallery-band")));
            p.rect_filled(band, theme::radius(6.0), theme::alpha(theme::ACCENT, 0.1));
            p.rect_stroke(band, theme::radius(6.0), Stroke::new(1.0, theme::alpha(theme::ACCENT, 0.7)), egui::StrokeKind::Inside);
            if !down {
                self.band = None;
            }
        }

        // The lifted cards go to Windows once they have lifted on screen, if the button is still down.
        if !self.carried.is_empty() && !self.started {
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.carried.clear();
            } else if now - self.lifted_at >= LIFT {
                let entries: Vec<&Entry> = self.carried.iter().filter_map(|k| order.iter().find(|o| &o.key == k)).collect();
                let files = files(&entries);
                if files.is_empty() {
                    self.carried.clear();
                } else {
                    let with_files: Vec<&Entry> = entries.into_iter().filter(|e| e.local.is_some()).collect();
                    drag_out::start(drag_out::Drag { image: drag_image(&with_files), files });
                    self.started = true;
                }
            } else {
                ctx.request_repaint();
            }
        }
        if let Some(outcome) = drag_out::take_outcome() {
            let n = self.carried.iter().filter(|k| order.iter().any(|o| &o.key == *k && o.local.is_some())).count();
            if outcome == Outcome::Copied {
                self.said = Some((copied(n, order, &self.carried), now));
            } else if outcome == Outcome::Failed {
                self.said = Some(("Windows could not start the drag".into(), now));
            }
            self.carried.clear();
            self.started = false;
        }
    }

    /// The bar at the bottom: how many are selected, them in a little stack, the hint, Select all and Clear; after a
    /// drag, what it copied.
    pub fn bar(&mut self, ui: &mut Ui, page: Rect, order: &[Entry], pictures: &mut Option<crate::worlds::Pictures>, hidden: bool) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let said = self.said.as_ref().filter(|(_, at)| now - at < SAID_FOR).map(|(s, _)| s.clone());
        if let Some((_, at)) = &self.said {
            if now - at < SAID_FOR {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAID_FOR - (now - at)));
            }
        }
        let shown = !hidden && (self.any() || said.is_some());
        let k = anim::toggle(&ctx, Id::unique("gallery-select-bar"), shown, Spring::BOUNCY);
        if k < 0.01 {
            return;
        }
        let n = self.keys.len();
        let count = Txt::new(if n == 1 { "1 selected".to_string() } else { format!("{n} selected") }, 14.0).weight(weight::SEMIBOLD);
        let hint_text = said.clone().unwrap_or_else(|| "Drag them into Discord, a folder or any app".into());
        let hint = Txt::new(&hint_text, 12.5).color(if said.is_some() { theme::ACCENT } else { theme::FG_3 });
        let all = Button::new("Select all").variant(Variant::Ghost).size(Size::Sm).id_salt("gallery-select-all");
        let thumbs = self.keys.len().min(3);
        let count_w = if self.any() { widgets::text_size(ui, &count).x } else { 0.0 };
        let narrow = page.width() < 760.0;
        let hint_w = if narrow && said.is_none() { 0.0 } else { widgets::text_size(ui, &hint).x };
        let all_w = if self.any() && n < order.len() { all.desired_width(ui) } else { 0.0 };
        let stack_w = if thumbs > 0 { 34.0 + 10.0 * (thumbs - 1) as f32 + 12.0 } else { 0.0 };
        let icon_w = if said.is_some() && !self.any() { 26.0 } else { 0.0 };
        let inner = stack_w
            + icon_w
            + count_w
            + if count_w > 0.0 && hint_w > 0.0 { 16.0 } else { 0.0 }
            + hint_w
            + if all_w > 0.0 { 12.0 + all_w } else { 0.0 }
            + if self.any() { 8.0 + 32.0 } else { 0.0 };
        let w = (inner + 28.0).min(page.width() - 48.0);
        let h = 52.0;
        let target = Rect::from_center_size(pos2(page.center().x, page.bottom() - 28.0 - h * 0.5), vec2(w, h));
        // Rises out of the bottom edge, growing from a pill.
        let rise = 1.0 - k.clamp(0.0, 1.2);
        let rect = Rect::from_center_size(target.center() + vec2(0.0, 40.0 * rise), vec2(w * (0.86 + 0.14 * k.min(1.0)), h));
        let id = Id::unique("gallery-select-bar-area");
        let mut clear = false;
        let mut select_all = false;
        egui::Area::new(id).order(Order::Foreground).fixed_pos(rect.min).fade_in(false).show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.multiply_opacity(k.clamp(0.0, 1.0));
            let p = ui.painter().clone();
            paint::shadow(&p, rect, h * 0.5, vec2(0.0, 14.0), 34.0, -6.0, theme::black(0.5));
            p.rect_filled(rect, theme::radius(h * 0.5), theme::surface_strong());
            p.rect_stroke(rect, theme::radius(h * 0.5), theme::hairline(), egui::StrokeKind::Inside);
            // Takes the clicks that land on it (not the cards under it).
            ui.interact(rect, id.with("body"), Sense::click());
            let mut x = rect.left() + 14.0;
            let cy = rect.center().y;
            if thumbs > 0 {
                let picked: Vec<&Entry> = order.iter().filter(|e| self.keys.contains(&e.key)).take(3).collect();
                for (i, e) in picked.iter().enumerate().rev() {
                    let r = Rect::from_center_size(pos2(x + 17.0 + 10.0 * i as f32, cy), vec2(34.0, 34.0));
                    let tex = e.picture.as_ref().and_then(|pic| pictures.as_mut()?.get(&ctx, pic));
                    p.rect_filled(r.expand(2.0), theme::radius(10.0), theme::surface_strong());
                    super::sessions::thumb_shape(&p, r, egui::CornerRadius::same(9), tex.as_ref(), 1.0);
                }
                x += stack_w;
            }
            if icon_w > 0.0 {
                widgets::icon(&p, pos2(x + 9.0, cy), icons::CHECK, 16.0, theme::ACCENT);
                x += icon_w;
            }
            if count_w > 0.0 {
                widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &count);
                x += count_w + 16.0;
            }
            if hint_w > 0.0 {
                let room = rect.right() - x - 14.0 - if all_w > 0.0 { 12.0 + all_w } else { 0.0 } - if self.any() { 40.0 } else { 0.0 };
                widgets::text_truncated(&p, pos2(x, cy), Align2::LEFT_CENTER, &hint, room.max(0.0));
            }
            let mut right = rect.right() - 10.0;
            if self.any() {
                let close = Rect::from_center_size(pos2(right - 16.0, cy), vec2(32.0, 32.0));
                let resp = ui.interact(close, id.with("clear"), Sense::click()).on_hover_text("Clear the selection (Esc)");
                let hover = anim::toggle(&ctx, id.with("clear-hover"), resp.hovered(), Spring::SNAPPY);
                p.circle_filled(close.center(), 16.0, theme::white(0.04 + 0.08 * hover));
                widgets::icon(&p, close.center(), icons::X, 15.0, theme::mix(theme::FG_3, theme::FG, hover));
                if widgets::activated(&resp) {
                    clear = true;
                }
                right = close.left() - 8.0;
            }
            if all_w > 0.0 && crate::pages::sharing::pressed(all.show_at(ui, Rect::from_min_size(pos2(right - all_w, cy - 16.0), vec2(all_w, 32.0)))) {
                select_all = true;
            }
        });
        if clear {
            self.clear();
        }
        if select_all {
            self.keys = order.iter().map(|e| e.key.clone()).collect();
        }
    }
}

/// What a card shows of the selection: the ring around it, how far it shrank, the check on its corner (visible on
/// hover, while selecting, and filled when chosen). `radius` is the card's corners'; a picture that is the whole card
/// (the tile layouts) is tinted to all its corners.
#[allow(clippy::too_many_arguments)]
pub fn paint_card(p: &egui::Painter, ctx: &egui::Context, key: &str, card: Rect, thumb: Rect, radius: f32, selected: bool, show_check: bool, hover: f32) -> f32 {
    let id = Id::unique(("gallery-select", key));
    let s = anim::toggle(ctx, id.with("on"), selected, Spring::SNAPPY);
    let pop = anim::toggle(ctx, id.with("pop"), selected, Spring::BOUNCY);
    let vis = anim::toggle(ctx, id.with("check"), show_check || selected || hover > 0.5, Spring::SOFT);
    if s > 0.01 {
        let ring = card.expand(3.0);
        paint::stroke_squircle(p, ring, theme::squircle(radius + 3.0), Stroke::new(2.5, theme::alpha(theme::ACCENT, s)));
        let whole = thumb.bottom() >= card.bottom() - 0.5;
        let corners = [radius, radius, if whole { radius } else { 0.0 }, if whole { radius } else { 0.0 }];
        orbiters_ui::feedback::fill_corners(p, thumb, corners, theme::alpha(theme::ACCENT, 0.1 * s));
    }
    if vis > 0.01 {
        let c = pos2(thumb.left() + 12.0 + CHECK * 0.5, thumb.top() + 12.0 + CHECK * 0.5);
        let r = CHECK * 0.5 * (0.7 + 0.3 * vis) * (1.0 + 0.12 * (pop - s).abs().min(0.3));
        p.circle_filled(c, r + 1.0, theme::black(0.35 * vis * (1.0 - s)));
        p.circle_stroke(c, r - 1.0, Stroke::new(2.0, theme::white(0.85 * vis * (1.0 - s))));
        if s > 0.01 {
            p.circle_filled(c, r * (0.6 + 0.4 * ease::out_cubic(s)), theme::alpha(theme::ACCENT, s.min(1.0)));
            widgets::icon(p, c, icons::CHECK, 14.0 * pop.clamp(0.0, 1.2), theme::alpha(theme::ON_ACCENT, s.min(1.0)));
        }
    }
    vis
}

/// The check's rect on a card's picture (presses there toggle the card).
pub fn check_rect(thumb: Rect) -> Rect {
    Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(CHECK, CHECK))
}

/// "Copied 3 pictures", "Copied a 3D capture's picture", "Copied 2 pictures and a 3D capture".
fn copied(n: usize, order: &[Entry], carried: &[String]) -> String {
    let entries: Vec<&Entry> = carried.iter().filter_map(|k| order.iter().find(|o| &o.key == k)).filter(|e| e.local.is_some()).collect();
    let pictures = entries.iter().filter(|e| matches!(e.local, Some(Local::Picture(_)))).count();
    let captures = n.saturating_sub(pictures);
    let part = |count: usize, one: &str, many: &str| match count {
        0 => None,
        1 => Some(one.to_string()),
        c => Some(format!("{c} {many}")),
    };
    let parts: Vec<String> = [part(pictures, "a picture", "pictures"), part(captures, "a 3D capture", "3D captures")].into_iter().flatten().collect();
    format!("Copied {}", parts.join(" and "))
}

/// The files to hand out: each picture as it is, each capture as the picture of its 3D view (made once, kept in the
/// cache under the capture folder's name).
fn files(entries: &[&Entry]) -> Vec<PathBuf> {
    let dir = orbiters_core::paths::cache_dir().join("dragged");
    entries
        .iter()
        .filter_map(|e| match e.local.as_ref()? {
            Local::Picture(p) => p.is_file().then(|| p.clone()),
            Local::Session(dir_of) => {
                let name = dir_of.file_name()?.to_string_lossy().into_owned();
                crate::capture_picture::png_file(dir_of, &dir, &name)
            }
        })
        .collect()
}

/// The drag image: the first three as a fanned stack of rounded pictures with a soft shadow, the count on a green
/// badge when there are several.
fn drag_image(entries: &[&Entry]) -> Option<drag_out::Image> {
    let pictures: Vec<image::RgbaImage> = entries
        .iter()
        .take(3)
        .filter_map(|e| crate::worlds::decode_local(e.picture.as_ref()?))
        .filter_map(|c| image::RgbaImage::from_raw(c.size[0] as u32, c.size[1] as u32, c.as_raw().to_vec()))
        .collect();
    let front = pictures.first()?;
    let n = entries.len();
    let (cw, ch) = card_size(front.width(), front.height());
    let pad = 22u32;
    let fan = 10u32 * (pictures.len() as u32 - 1);
    let (w, h) = (cw + pad * 2 + fan + 14, ch + pad * 2 + fan + 14);
    let mut canvas = Canvas::new(w, h);
    // The cards behind first, each further up and to the right, darker.
    for (i, picture) in pictures.iter().enumerate().rev() {
        let off = 10 * i as u32;
        let (x, y) = (pad + off, pad + 14 + fan - off);
        let (pw, ph) = if i == 0 { (cw, ch) } else { (cw, ch) };
        let scaled = image::imageops::resize(picture, pw, ph, image::imageops::FilterType::Triangle);
        canvas.shadow(x as f32, y as f32 + 6.0, pw as f32, ph as f32, 12.0, 10.0, 0.42);
        let shade = 1.0 - 0.18 * i as f32;
        canvas.picture(&scaled, x as f32, y as f32, 12.0, shade, 0.94);
        canvas.outline(x as f32, y as f32, pw as f32, ph as f32, 12.0, [255, 255, 255], 0.22);
    }
    if n > 1 {
        let (bx, by) = ((pad + cw) as f32 - 2.0, (pad + 14 + fan) as f32 + 2.0);
        canvas.badge(bx, by, &n.to_string());
    }
    // The pointer holds the front card a little below its middle.
    Some(drag_out::Image { width: w, height: h, rgba: canvas.rgba, cursor: [(pad + cw / 2) as i32, (pad + 14 + fan + ch / 2 + 10) as i32] })
}

/// A card of the stack, 168 points wide at most and as tall as its picture allows (90 to 168).
fn card_size(w: u32, h: u32) -> (u32, u32) {
    let aspect = w as f32 / h.max(1) as f32;
    let cw = 168.0f32;
    let ch = (cw / aspect).clamp(90.0, 168.0);
    (cw as u32, ch as u32)
}

/// A straight-alpha RGBA canvas painted with coverage (antialiased rounded corners), over-blended.
struct Canvas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self { w, h, rgba: vec![0; (w * h * 4) as usize] }
    }

    fn blend(&mut self, x: u32, y: u32, c: [f32; 3], a: f32) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let d = &mut self.rgba[i..i + 4];
        let da = d[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            let v = (c[k] * a + d[k] as f32 / 255.0 * da * (1.0 - a)) / oa;
            d[k] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (oa * 255.0).round() as u8;
    }

    /// Coverage of a rounded rectangle at a pixel's centre.
    fn inside(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let qx = (px - cx).abs() - (w * 0.5 - r);
        let qy = (py - cy).abs() - (h * 0.5 - r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
        (0.5 - outside).clamp(0.0, 1.0)
    }

    fn picture(&mut self, img: &image::RgbaImage, x: f32, y: f32, r: f32, shade: f32, alpha: f32) {
        for (ix, iy, p) in img.enumerate_pixels() {
            let (px, py) = (x + ix as f32 + 0.5, y + iy as f32 + 0.5);
            let cov = Self::inside(px, py, x, y, img.width() as f32, img.height() as f32, r);
            let c = [p[0] as f32 / 255.0 * shade, p[1] as f32 / 255.0 * shade, p[2] as f32 / 255.0 * shade];
            self.blend((x as u32) + ix, (y as u32) + iy, c, cov * alpha * p[3] as f32 / 255.0);
        }
    }

    fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: [u8; 3], alpha: f32) {
        let c = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
        for py in (y as u32)..(y + h).ceil() as u32 {
            for px in (x as u32)..(x + w).ceil() as u32 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let ring = Self::inside(fx, fy, x, y, w, h, r) - Self::inside(fx, fy, x + 1.0, y + 1.0, w - 2.0, h - 2.0, r - 1.0);
                self.blend(px, py, c, ring.max(0.0) * alpha);
            }
        }
    }

    /// A soft shadow: the rounded rectangle's coverage, feathered over `blur` pixels.
    fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, blur: f32, alpha: f32) {
        let (x0, y0) = ((x - blur).max(0.0) as u32, (y - blur).max(0.0) as u32);
        let (x1, y1) = (((x + w + blur) as u32).min(self.w), ((y + h + blur) as u32).min(self.h));
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        for py in y0..y1 {
            for px in x0..x1 {
                let qx = (px as f32 + 0.5 - cx).abs() - (w * 0.5 - r);
                let qy = (py as f32 + 0.5 - cy).abs() - (h * 0.5 - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                let t = (1.0 - (d + blur * 0.3) / blur).clamp(0.0, 1.0);
                self.blend(px, py, [0.0, 0.0, 0.0], alpha * t * t * (3.0 - 2.0 * t));
            }
        }
    }

    /// The count on a green disc with a dark rim, centred on (`x`, `y`).
    fn badge(&mut self, x: f32, y: f32, text: &str) {
        let digits: Vec<&[&str; 7]> = text.chars().filter_map(|c| c.to_digit(10)).map(|d| &DIGITS[d as usize]).collect();
        let tw = digits.len() as f32 * 6.0 - 1.0;
        let r = (tw * 0.5 + 8.0).max(13.0);
        let (hw, hh) = (r.max(tw * 0.5 + 8.0), 13.0);
        let accent = [0.0, 218.0 / 255.0, 109.0 / 255.0];
        let ink = [5.0 / 255.0, 22.0 / 255.0, 12.0 / 255.0];
        for py in (y - hh - 3.0) as u32..(y + hh + 3.0) as u32 {
            for px in (x - hw - 3.0) as u32..(x + hw + 3.0) as u32 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let rim = Self::inside(fx, fy, x - hw - 2.0, y - hh - 2.0, hw * 2.0 + 4.0, hh * 2.0 + 4.0, hh + 2.0);
                self.blend(px, py, [0.035, 0.035, 0.043], rim);
                let disc = Self::inside(fx, fy, x - hw, y - hh, hw * 2.0, hh * 2.0, hh);
                self.blend(px, py, accent, disc);
            }
        }
        // Digits drawn at twice their 5x7 grid, in the badge's dark ink.
        let (sx, sy) = (x - tw, y - 7.0);
        for (i, rows) in digits.iter().enumerate() {
            for (ry, row) in rows.iter().enumerate() {
                for (rx, cell) in row.chars().enumerate() {
                    if cell != '#' {
                        continue;
                    }
                    let (cx, cy) = (sx + (i as f32 * 6.0 + rx as f32) * 2.0, sy + ry as f32 * 2.0);
                    for py in cy as u32..(cy + 2.0) as u32 {
                        for px in cx as u32..(cx + 2.0) as u32 {
                            self.blend(px, py, ink, 1.0);
                        }
                    }
                }
            }
        }
    }
}

/// 5x7 digits for the count badge.
const DIGITS: [[&str; 7]; 10] = [
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    [".###.", "#....", "#....", "####.", "#...#", "#...#", ".###."],
    ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, picture: bool) -> Entry {
        let mut e = crate::controller::shares::remote_entry(orbiters_core::shares::Item::default());
        e.key = key.into();
        e.local = Some(if picture { Local::Picture(PathBuf::from(format!("{key}.png"))) } else { Local::Session(PathBuf::from(key)) });
        e
    }

    #[test]
    fn what_a_drag_copied_reads_naturally() {
        let order = vec![entry("a", true), entry("b", true), entry("c", false)];
        let keys = |ks: &[&str]| ks.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(copied(1, &order, &keys(&["a"])), "Copied a picture");
        assert_eq!(copied(2, &order, &keys(&["a", "b"])), "Copied 2 pictures");
        assert_eq!(copied(1, &order, &keys(&["c"])), "Copied a 3D capture");
        assert_eq!(copied(3, &order, &keys(&["a", "b", "c"])), "Copied 2 pictures and a 3D capture");
    }

    #[test]
    fn stack_cards_keep_their_shape_within_bounds() {
        assert_eq!(card_size(1920, 1080), (168, 94));
        assert_eq!(card_size(1080, 1920), (168, 168));
        assert_eq!(card_size(4000, 400), (168, 90));
    }

    #[test]
    fn the_badge_and_pictures_paint_inside_the_canvas() {
        let mut c = Canvas::new(80, 60);
        c.badge(40.0, 30.0, "12");
        let img = image::RgbaImage::from_pixel(30, 20, image::Rgba([200, 100, 50, 255]));
        c.picture(&img, 5.0, 5.0, 6.0, 1.0, 1.0);
        c.shadow(5.0, 5.0, 30.0, 20.0, 6.0, 8.0, 0.4);
        // The badge's green middle and its dark digits are there; the corner of the rounded picture stays clear.
        assert!(c.rgba.chunks_exact(4).any(|p| p[1] > 200 && p[0] < 20));
        assert!(c.rgba.chunks_exact(4).any(|p| p[3] == 255 && p[1] < 40 && p[0] < 20));
        assert_eq!(c.rgba[((5 * 80 + 5) * 4 + 3) as usize] < 160, true);
    }
}
