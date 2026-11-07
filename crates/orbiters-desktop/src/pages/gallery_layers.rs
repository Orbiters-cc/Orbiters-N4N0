//! VRChat's multi-layer pictures in Photos & captures ([`Layer`]: the camera's Multi Layer mode saves the world, the
//! people and the interface each as a PNG beside the picture, the listing groups them under it): the badge a card with
//! layers carries, the checkerboard under what is transparent, and the picture window's layer strip ([`LayerView`]):
//! the picture itself, or its layers stacked over a checkerboard, each shown or hidden by its eye. A chosen layer is
//! what the window is about: Share shares that file (its own upload and grants), the picture otherwise.
use crate::controller::shares::{Entry, Layer};
use crate::worlds::{Pictures, THUMB_WIDTH};
use egui::{pos2, vec2, Align2, Color32, ColorImage, Id, Painter, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui};
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::{
    anim::{self, Spring},
    feedback, icons, paint,
    theme::{self, weight, Txt},
    widgets,
};
use std::collections::BTreeSet;

/// How wide the window decodes a layer (as its picture).
const LARGE: u32 = 2048;
/// A checkerboard square, in points.
const SQUARE: f32 = 8.0;

/// A checkerboard under a picture with transparency, in `rect` with squircle corners `radii` (circular radii,
/// top-left first, as [`feedback::cover`]'s), faded by `alpha`.
pub fn checkerboard(p: &Painter, rect: Rect, radii: [f32; 4], alpha: f32) {
    let ctx = p.ctx().clone();
    let id = Id::unique("gallery-checkerboard");
    let tex = ctx.data(|d| d.get_temp::<TextureHandle>(id)).unwrap_or_else(|| {
        let (light, dark) = (Color32::from_gray(66), Color32::from_gray(46));
        let t = ctx.load_texture("gallery-checkerboard", ColorImage::new([2, 2], vec![light, dark, dark, light]), TextureOptions::NEAREST_REPEAT);
        ctx.data_mut(|d| d.insert_temp(id, t.clone()));
        t
    });
    let uv = Rect::from_min_size(Pos2::ZERO, vec2(rect.width() / (SQUARE * 2.0), rect.height() / (SQUARE * 2.0)));
    paint::squircle_texture(p, rect, radii.map(theme::squircle), tex.id(), uv, Color32::WHITE.gamma_multiply(alpha.clamp(0.0, 1.0)));
}

/// What a card with layers says on its picture: "3 layers", or "Player layer" for a layer alone.
pub fn label(e: &Entry) -> Option<String> {
    match e.layers() {
        [] => None,
        [one] if !e.has_composite() => Some(tr!("{name} layer", name = one.name)),
        many => Some(tr_n("{count} layer", "{count} layers", many.len() as u64)),
    }
}

/// A small label with an icon on a picture (as the cards' badges); returns its width.
pub fn badge(p: &Painter, ui: &Ui, at: Pos2, glyph: &str, text: &str, right: bool) -> f32 {
    let t = Txt::new(text, 10.5).weight(weight::SEMIBOLD).tracking(0.025).color(theme::FG);
    let w = widgets::text_size(ui, &t).x + 20.0 + 15.0;
    let r = if right { Rect::from_min_size(pos2(at.x - w, at.y), vec2(w, 22.0)) } else { Rect::from_min_size(at, vec2(w, 22.0)) };
    p.rect_filled(r, theme::radius(11.0), theme::black(0.55));
    widgets::icon(p, pos2(r.left() + 16.0, r.center().y), glyph, 12.0, theme::ACCENT);
    widgets::text(p, pos2(r.left() + 25.0, r.center().y), Align2::LEFT_CENTER, &t);
    w
}

/// A multi-layer picture's layers in its window: which one the window is about and how they are shown.
#[derive(Default)]
pub struct LayerView {
    /// The entry the view was set up for (the chosen layer's while one is): another one shown means the window went to
    /// another item, and the view starts again.
    shown_key: String,
    /// The multi-layer picture, while one of its layers is chosen (the window shows that layer's entry).
    parent: Option<Entry>,
    /// The layer chosen: None for the picture itself.
    chosen: Option<usize>,
    /// The layers are shown stacked over a checkerboard (instead of the picture), those in `hidden` left out.
    stack: bool,
    hidden: BTreeSet<usize>,
}

impl LayerView {
    /// The multi-layer picture: the parent while a layer is chosen, else the entry shown.
    pub fn layered<'a>(&'a self, shown: &'a Entry) -> &'a Entry {
        self.parent.as_ref().unwrap_or(shown)
    }

    /// The chosen layer's name, while one is.
    pub fn chosen_name(&self) -> Option<&str> {
        Some(self.parent.as_ref()?.layers().get(self.chosen?)?.name.as_str())
    }

    /// The key of the gallery card the window belongs to (its arrows, its card, its landing).
    pub fn card_key<'a>(&'a self, shown: &'a Entry) -> &'a str {
        &self.layered(shown).key
    }

    /// Set up for the entry shown, once each time the window shows another item: the picture, or, when its picture is
    /// gone, its layers stacked.
    fn follow(&mut self, shown: &Entry) {
        if self.shown_key == shown.key {
            return;
        }
        *self = Self { shown_key: shown.key.clone(), stack: !shown.layers().is_empty() && !shown.has_composite(), ..Self::default() };
    }

    /// Whether the window shows the stack (painted by [`LayerView::paint`]) instead of its picture.
    pub fn stacked(&mut self, shown: &Entry) -> bool {
        self.follow(shown);
        self.stack && !self.layered(shown).layers().is_empty()
    }

    /// The stack over the window's picture: the checkerboard, then each layer shown, back to front (the large one
    /// once decoded, its thumbnail until then). Nothing when the picture itself is shown.
    pub fn paint(&mut self, p: &Painter, rect: Rect, radii: [f32; 4], alpha: f32, pictures: &mut Pictures, shown: &Entry) {
        if !self.stacked(shown) {
            return;
        }
        let ctx = p.ctx().clone();
        checkerboard(p, rect, radii, alpha);
        let layers: Vec<Layer> = self.layered(shown).layers().to_vec();
        for (i, l) in layers.iter().enumerate() {
            if self.hidden.contains(&i) {
                continue;
            }
            let tex = pictures.get(&ctx, &l.picture(LARGE)).or_else(|| pictures.get(&ctx, &l.picture(THUMB_WIDTH)));
            if let Some(t) = tex {
                feedback::cover(p, rect, radii, &t, 1.0, alpha);
            }
        }
    }

    /// The strip of layers over the picture's bottom left: the picture, then each layer (its thumbnail over a
    /// checkerboard, its name, its eye). A press on one chooses it (shown alone; the picture shows the picture), an eye
    /// shows or hides that layer in the stack. Returns the entry the window is about from now on when that changed.
    /// `room` is how wide it may be; `enabled` false while a share of the one shown runs.
    pub fn strip(&mut self, ui: &mut Ui, picture: Rect, room: f32, shown: &Entry, pictures: &mut Pictures, enabled: bool) -> Option<Entry> {
        self.follow(shown);
        let layered = self.layered(shown).clone();
        let layers = layered.layers();
        if layers.is_empty() {
            return None;
        }
        let ctx = ui.ctx().clone();
        let id = Id::unique(("gallery-layer-strip", &layered.key));
        let composite = layered.has_composite();
        let name = |s: &str| Txt::new(s, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
        // Each chip: the picture (an icon, its word), each layer (its thumbnail, its name, its eye).
        let mut chips: Vec<(Option<usize>, f32)> = Vec::new();
        if composite {
            chips.push((None, 12.0 + 16.0 + 6.0 + widgets::text_size(ui, &name(tr("Picture"))).x + 12.0));
        }
        for (i, l) in layers.iter().enumerate() {
            chips.push((Some(i), 4.0 + 24.0 + 7.0 + widgets::text_size(ui, &name(&l.name)).x + 4.0 + 26.0 + 3.0));
        }
        let total: f32 = chips.iter().map(|c| c.1).sum::<f32>() + 8.0 + 4.0 * (chips.len() - 1) as f32;
        // Too narrow for the names: the thumbnails and the eyes alone.
        let compact = total > room;
        if compact {
            for c in &mut chips {
                c.1 = if c.0.is_some() { 4.0 + 24.0 + 4.0 + 26.0 + 3.0 } else { 40.0 };
            }
        }
        let total: f32 = chips.iter().map(|c| c.1).sum::<f32>() + 8.0 + 4.0 * (chips.len() - 1) as f32;
        let bar = Rect::from_min_size(pos2(picture.left() + 16.0, picture.bottom() - 16.0 - 40.0), vec2(total, 40.0));
        let p = ui.painter().clone();
        let mut fade = p.clone();
        if !enabled {
            fade.multiply_opacity(0.5);
        }
        fade.rect_filled(bar, theme::radius(20.0), theme::black(0.6));
        // Takes the presses that land on it (not the picture under it).
        ui.interact(bar, id.with("bar"), Sense::click());
        let selected = |c: Option<usize>| c == self.chosen;
        let mut x = bar.left() + 4.0;
        let mut choose: Option<Option<usize>> = None;
        let mut toggle: Option<usize> = None;
        let rects: Vec<Rect> = chips
            .iter()
            .map(|c| {
                let r = Rect::from_min_size(pos2(x, bar.top() + 4.0), vec2(c.1, 32.0));
                x += c.1 + 4.0;
                r
            })
            .collect();
        // The chosen one's light slides between them.
        if let Some(at) = chips.iter().position(|c| selected(c.0)) {
            let [l, w] = anim::spring(&ctx, id.with("pill"), [rects[at].left() - bar.left(), rects[at].width()], Spring::BOUNCY);
            fade.rect_filled(Rect::from_min_size(pos2(bar.left() + l, bar.top() + 4.0), vec2(w, 32.0)), theme::radius(16.0), theme::white(0.16));
        }
        for (n, (chip, _)) in chips.iter().enumerate() {
            let r = rects[n];
            let eye = Rect::from_min_size(pos2(r.right() - 3.0 - 26.0, r.top() + 3.0), vec2(26.0, 26.0));
            let body = if chip.is_some() { Rect::from_min_max(r.min, pos2(eye.left(), r.bottom())) } else { r };
            let hint = match chip {
                None => tr("The picture, all layers in one").to_string(),
                Some(i) => tr!("Only the {layer} layer (Share shares it)", layer = layers[*i].name),
            };
            let resp = ui.interact(body, id.with(("chip", n)), if enabled { Sense::click() } else { Sense::hover() }).on_hover_text(hint);
            let hover = anim::toggle(&ctx, id.with(("hover", n)), resp.hovered() && enabled, Spring::SNAPPY);
            if hover > 0.01 && !selected(*chip) {
                fade.rect_filled(r, theme::radius(16.0), theme::white(0.07 * hover));
            }
            if enabled && widgets::activated(&resp) {
                choose = Some(*chip);
            }
            match chip {
                None => {
                    widgets::icon(&fade, pos2(r.left() + 12.0 + 8.0, r.center().y), icons::IMAGES, 15.0, theme::FG);
                    if !compact {
                        widgets::text(&fade, pos2(r.left() + 12.0 + 16.0 + 6.0, r.center().y), Align2::LEFT_CENTER, &name(tr("Picture")));
                    }
                }
                Some(i) => {
                    let l = &layers[*i];
                    let thumb = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 4.0), vec2(24.0, 24.0));
                    let radii = [7.0; 4];
                    checkerboard(&fade, thumb, radii, 1.0);
                    if let Some(t) = pictures.get(&ctx, &l.picture(THUMB_WIDTH)) {
                        feedback::cover(&fade, thumb, radii, &t, 1.0, 1.0);
                    }
                    let hidden = self.stack && self.hidden.contains(i);
                    if !compact {
                        let mut t = name(&l.name);
                        if hidden {
                            t = t.color(theme::FG_3);
                        }
                        widgets::text(&fade, pos2(thumb.right() + 7.0, r.center().y), Align2::LEFT_CENTER, &t);
                    }
                    let er = ui.interact(eye, id.with(("eye", n)), if enabled { Sense::click() } else { Sense::hover() });
                    let er = er.on_hover_text(if hidden { tr!("Show the {layer} layer", layer = l.name) } else { tr!("Hide the {layer} layer", layer = l.name) });
                    let eh = anim::toggle(&ctx, id.with(("eye-hover", n)), er.hovered() && enabled, Spring::SNAPPY);
                    fade.circle_filled(eye.center(), 13.0, theme::white(0.1 * eh));
                    let glyph = if hidden { icons::EYE_SLASH } else { icons::EYE };
                    widgets::icon(&fade, eye.center(), glyph, 14.0, theme::mix(if hidden { theme::FG_3 } else { theme::FG_2 }, theme::FG, eh));
                    if enabled && widgets::activated(&er) {
                        toggle = Some(*i);
                    }
                }
            }
        }
        if let Some(i) = toggle {
            self.toggle(i);
        }
        self.choose(shown, choose?)
    }

    /// Shows or hides a layer in the stack (from the picture, the stack with every layer but this one).
    pub fn toggle(&mut self, i: usize) {
        if !self.stack {
            self.stack = true;
            self.hidden.clear();
        }
        if !self.hidden.remove(&i) {
            self.hidden.insert(i);
        }
    }

    /// Chooses a layer (shown alone over the checkerboard) or, with None, the picture; returns the entry the window is
    /// about from now on when that changed.
    pub fn choose(&mut self, shown: &Entry, choice: Option<usize>) -> Option<Entry> {
        self.follow(shown);
        if choice == self.chosen || self.layered(shown).layers().is_empty() {
            return None;
        }
        let composite = self.layered(shown).has_composite();
        // The layer left keeps what its share did.
        if let (Some(old), Some(parent)) = (self.chosen, self.parent.as_mut()) {
            if let Some(l) = parent.layers.get_mut(old) {
                l.shared = shown.remote.clone();
            }
        }
        let parent = self.parent.take().unwrap_or_else(|| shown.clone());
        self.chosen = choice;
        let next = match choice {
            None => {
                self.stack = !composite;
                self.hidden.clear();
                parent
            }
            Some(i) => {
                // Alone over the checkerboard; the eyes bring the others back.
                self.stack = true;
                self.hidden = (0..parent.layers().len()).filter(|j| *j != i).collect();
                let entry = parent.layer_entry(i)?;
                self.parent = Some(parent);
                entry
            }
        };
        self.shown_key = next.key.clone();
        Some(next)
    }
}
