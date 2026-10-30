//! The chosen layer's controls on the editor's right: its opacity and where it acts (everywhere, in a circle or along
//! a band), then its own: light's and colour's sliders, a curve to drag per channel over the picture's histogram, the
//! vignette's and the aberration's strength, the comic's tones, ink and dots, the blur's kind and size, the text's
//! words, font (the built-in ones, then this computer's, each shown in itself), size, style and colour. Crop and rotate
//! has its shapes, straighten, quarter turns and flips.

use super::{glyph, layers, tint, widgets as w, Editor, Selected};
use crate::pages::Job;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_ui::photo::text::FontInfo;
use orbiters_ui::photo::{self, render, Aspect, BlurKind, Channel, Effect, Kind, Mask, TextStyle};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Chip, IconButton, SegOption, Size};
use orbiters_ui::icons;
use std::collections::HashMap;

/// The fonts offered to text layers, and each one's name drawn in itself.
#[derive(Default)]
pub(super) struct Fonts {
    installed: Option<Job<Vec<FontInfo>>>,
    list: Vec<FontInfo>,
    query: String,
    previews: HashMap<String, egui::TextureHandle>,
}

impl Fonts {
    fn ensure(&mut self, ctx: &egui::Context) {
        if self.installed.is_none() {
            self.list = photo::text::bundled();
            self.installed = Some(Job::spawn(ctx, photo::text::system));
        }
        if let Some(found) = self.installed.as_mut().and_then(|j| j.take()) {
            self.list.extend(found);
        }
    }

    /// The name of `font` drawn in it (white, about 20 points high), made the first time it shows.
    fn preview(&mut self, ctx: &egui::Context, font: &FontInfo) -> Option<egui::TextureHandle> {
        if let Some(t) = self.previews.get(&font.key) {
            return Some(t.clone());
        }
        let ppp = ctx.pixels_per_point();
        let text = photo::Text { value: font.name.clone(), font: font.key.clone(), size: 0.2, style: TextStyle::Clean, color: [255, 255, 255], center: [0.5, 0.5] };
        let d = photo::text::draw(&text, (100.0 * ppp) as u32)?;
        let rgba: Vec<u8> = d
            .pixels
            .iter()
            .flat_map(|p| {
                let a = p[3].clamp(0.0, 1.0);
                let c = |v: f32| if a > 0.0 { (photo::to_srgb(v / a).clamp(0.0, 1.0) * 255.0) as u8 } else { 0 };
                [c(p[0]), c(p[1]), c(p[2]), (a * 255.0) as u8]
            })
            .collect();
        let img = egui::ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &rgba);
        let t = ctx.load_texture(format!("editor-font-{}", font.key), img, egui::TextureOptions::LINEAR);
        self.previews.insert(font.key.clone(), t.clone());
        Some(t)
    }
}

pub(super) fn show(ed: &mut Editor, ui: &mut Ui, rect: Rect) {
    widgets::panel(ui.painter(), rect, theme::CARD_RADIUS);
    let inner = rect.shrink2(vec2(16.0, 14.0));
    let mut pu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(ed.id.with("controls")));
    pu.spacing_mut().item_spacing = vec2(0.0, 0.0);
    egui::ScrollArea::vertical().id_salt(ed.id.with(("controls-scroll", format!("{:?}", ed.selected)))).auto_shrink([false, false]).show(&mut pu, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // Clear of the scroll bar.
        ui.set_width(inner.width() - 14.0);
        match ed.selected {
            Selected::Nothing => nothing(ed, ui),
            Selected::Crop => crop(ed, ui),
            Selected::Source(i) => source(ed, ui, i),
            Selected::Layer(..) => layer(ed, ui),
        }
        ui.add_space(16.0);
    });
}

fn header(ui: &mut Ui, glyph: &str, color: Color32, title: &str, sub: &str) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(Txt::new(sub, 12.0).color(theme::FG_3).line_height(17.0).job(w - 46.0));
    let h = 22.0 + galley.size().y + 8.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h.max(44.0)), Sense::hover());
    let p = ui.painter();
    let tile = Rect::from_min_size(r.min + vec2(0.0, 2.0), vec2(34.0, 34.0));
    p.rect_filled(tile, theme::radius(11.0), theme::alpha(color, 0.16));
    widgets::icon(p, tile.center(), glyph, 17.0, color);
    widgets::text_truncated(p, pos2(tile.right() + 12.0, r.top() + 10.0), Align2::LEFT_CENTER, &Txt::new(title, 15.5).weight(weight::SEMIBOLD), w - 46.0);
    p.galley(pos2(tile.right() + 12.0, r.top() + 22.0), galley, theme::FG_3);
    ui.add_space(10.0);
}

fn paragraph(ui: &mut Ui, text: &str) {
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(theme::FG_3).line_height(18.0).job(ui.available_width()));
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, theme::FG_3);
}

fn nothing(ed: &mut Editor, ui: &mut Ui) {
    header(ui, icons::SLIDERS_HORIZONTAL, theme::ACCENT, "Edit a copy", "The picture itself is never changed: saving writes a copy beside it, with its edit, which opens again here.");
    let size = ed.loaded.as_ref().map(|l| format!("{} × {} pixels", l.full.0, l.full.1)).unwrap_or_else(|| "Reading the picture…".into());
    paragraph(ui, &size);
    if !ed.doc.sources.is_empty() {
        ui.add_space(8.0);
        let names: Vec<String> = ed.doc.sources.iter().rev().map(|s| layers::source_name(&s.name)).collect();
        paragraph(ui, &format!("A multi-layer picture from VRChat's camera: {}. Each has its own effects, under the whole picture's.", names.join(", ")));
    }
    w::label(ui, "Start with");
    let quick = [Kind::Light, Kind::Color, Kind::Comic, Kind::Text];
    let widths: Vec<f32> = quick.iter().map(|k| Chip::new(k.name()).icon(glyph(*k)).width(ui)).collect();
    let (pos, h) = widgets::flow(&widths, ui.available_width(), widgets::CHIP_H, 8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let title = ed.title.clone();
    for ((k, p), cw) in quick.iter().zip(pos).zip(widths) {
        let cr = Rect::from_min_size(r.min + p, vec2(cw, widgets::CHIP_H));
        if Chip::new(k.name()).icon(glyph(*k)).id_salt(("quick", k.name())).show_at(ui, cr).pressed {
            layers::add(ed, *k, &title);
        }
    }
    ui.add_space(14.0);
    paragraph(ui, "Ctrl+Z undoes, Ctrl+Shift+Z redoes, Delete removes the chosen layer. Hold the eye at the top to see the picture as it was.");
}

fn crop(ed: &mut Editor, ui: &mut Ui) {
    header(ui, icons::CROP, theme::ACCENT, "Crop and rotate", "Drag the corners or the edges on the picture, or inside to move the crop. Straightening keeps the crop inside the picture.");
    let full = ed.loaded.as_ref().map_or((16, 9), |l| l.full);
    let g = &mut ed.doc.geometry;
    let (tw, th) = photo::geometry::turned_size(full.0, full.1, g.turns);
    w::label(ui, "Shape");
    let widths: Vec<f32> = Aspect::ALL.iter().map(|a| Chip::new(a.name()).selected(g.aspect == *a).width(ui)).collect();
    let (pos, h) = widgets::flow(&widths, ui.available_width(), widgets::CHIP_H, 8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    for ((a, p), cw) in Aspect::ALL.iter().zip(pos).zip(widths) {
        let cr = Rect::from_min_size(r.min + p, vec2(cw, widgets::CHIP_H));
        if Chip::new(a.name()).selected(g.aspect == *a).id_salt(("aspect", a.name())).show_at(ui, cr).pressed {
            g.aspect = *a;
            // The largest crop of that shape (the original's after a quarter turn is its turned shape).
            if let Some(ratio) = a.ratio(tw as f32 / th.max(1) as f32) {
                g.crop = photo::geometry::with_ratio(photo::Crop::FULL, ratio, tw as f32, th as f32);
            }
        }
    }
    ui.add_space(10.0);
    w::slider(ui, "straighten", "Straighten", &mut g.straighten, -45.0..=45.0, 0.0, |v| format!("{v:.1}°"));
    ui.add_space(6.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let buttons = [
        (icons::ARROW_ARC_LEFT, "Turn left"),
        (icons::ARROW_ARC_RIGHT, "Turn right"),
        (icons::FLIP_HORIZONTAL, "Flip across"),
        (icons::FLIP_VERTICAL, "Flip down"),
    ];
    for (i, (glyph, label)) in buttons.iter().enumerate() {
        let r = Rect::from_min_size(pos2(row.left() + i as f32 * 48.0, row.top()), vec2(40.0, 40.0));
        if widgets::activated(&IconButton::new(glyph, label).size(40.0, 18.0).backdrop(theme::white(0.05)).show_at(ui, r)) {
            match i {
                0 | 1 => {
                    g.turns = (g.turns + if i == 0 { 3 } else { 1 }) % 4;
                    // The crop turns with the picture.
                    let c = g.crop;
                    g.crop = if i == 1 { photo::Crop { x: 1.0 - c.y - c.h, y: c.x, w: c.h, h: c.w } } else { photo::Crop { x: c.y, y: 1.0 - c.x - c.w, w: c.h, h: c.w } };
                }
                2 => g.flip_h = !g.flip_h,
                _ => g.flip_v = !g.flip_v,
            }
        }
    }
    ui.add_space(12.0);
    let b = Button::new("Reset").icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).enabled(!g.is_identity() || g.aspect != Aspect::Free).id_salt("crop-reset");
    let bw = b.desired_width(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
        *g = photo::Geometry::default();
    }
    ui.add_space(12.0);
    let (ow, oh) = g.output_size(full.0, full.1);
    paragraph(ui, &format!("The copy will be {ow} × {oh} pixels."));
}

fn source(ed: &mut Editor, ui: &mut Ui, i: usize) {
    let Some(s) = ed.doc.sources.get_mut(i) else { return };
    let name = layers::source_name(&s.name);
    let about = match s.name.to_ascii_lowercase().as_str() {
        "environment" => "The world, as VRChat's camera saved it apart from the people.",
        "player" | "players" | "users" => "The people, transparent around them: blur the world and keep them sharp, or give them the comic look alone.",
        "ui" => "The interface VRChat drew over the picture, transparent elsewhere.",
        _ => "The picture VRChat saved with its layers, all of them in one.",
    };
    header(ui, layers::source_glyph(&s.name), theme::FG_2, &name, about);
    w::switch(ui, ("source-visible", i), "Shown", &mut s.visible);
    ui.add_space(4.0);
    w::slider(ui, ("source-opacity", i), "Opacity", &mut s.opacity, 0.0..=1.0, 1.0, w::percent);
    ui.add_space(12.0);
    let b = Button::new("Add an effect to it").icon(icons::PLUS).size(Size::Sm).id_salt(("source-add", i));
    let bw = b.desired_width(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
        ed.adding = true;
    }
}

fn layer(ed: &mut Editor, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let histogram = ed.histogram.clone();
    let id = ed.id;
    let Some(kind) = ed.layer().map(|l| l.effect.kind()) else {
        ed.selected = Selected::Nothing;
        return;
    };
    header(ui, glyph(kind), tint(kind), kind.name(), kind.about());
    // Move, copy, remove.
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let acts = [(icons::ARROW_UP, "Move up"), (icons::ARROW_DOWN, "Move down"), (icons::COPY, "Duplicate"), (icons::TRASH, "Remove")];
    for (i, (glyph, label)) in acts.iter().enumerate() {
        let r = Rect::from_min_size(pos2(row.left() + i as f32 * 44.0, row.top()), vec2(36.0, 36.0));
        let mut b = IconButton::new(glyph, label).size(36.0, 16.0).backdrop(theme::white(0.05));
        if i == 3 {
            b = b.danger();
        }
        if widgets::activated(&b.show_at(ui, r)) {
            match i {
                0 => layers::nudge_selected(ed, 1),
                1 => layers::nudge_selected(ed, -1),
                2 => layers::duplicate_selected(ed),
                _ => {
                    layers::delete_selected(ed);
                    return;
                }
            }
        }
    }
    ui.add_space(8.0);
    let selected = ed.selected;
    let Some(l) = super::find_mut(&mut ed.doc, selected) else { return };
    let lid = l.id;
    w::slider(ui, ("opacity", lid), "Opacity", &mut l.opacity, 0.0..=1.0, 1.0, w::percent);
    match &mut l.effect {
        Effect::Light(v) => {
            w::label(ui, "Light");
            w::slider(ui, ("exposure", lid), "Exposure", &mut v.exposure, -4.0..=4.0, 0.0, |v| format!("{v:+.2} EV"));
            w::slider(ui, ("brightness", lid), "Brightness", &mut v.brightness, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("contrast", lid), "Contrast", &mut v.contrast, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("highlights", lid), "Highlights", &mut v.highlights, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("shadows", lid), "Shadows", &mut v.shadows, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("black", lid), "Black point", &mut v.black_point, -0.25..=0.5, 0.0, w::signed);
        }
        Effect::Color(v) => {
            w::label(ui, "White balance");
            w::slider(ui, ("temperature", lid), "Temperature", &mut v.temperature, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("tint", lid), "Tint", &mut v.tint, -1.0..=1.0, 0.0, w::signed);
            w::label(ui, "Colour");
            w::slider(ui, ("saturation", lid), "Saturation", &mut v.saturation, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("vibrance", lid), "Vibrance", &mut v.vibrance, -1.0..=1.0, 0.0, w::signed);
            w::slider(ui, ("hue", lid), "Hue", &mut v.hue, -180.0..=180.0, 0.0, |v| format!("{v:+.0}°"));
        }
        Effect::Curves(c) => {
            ui.add_space(8.0);
            let options = [SegOption::new(Channel::Rgb, "RGB"), SegOption::new(Channel::Red, "Red"), SegOption::new(Channel::Green, "Green"), SegOption::new(Channel::Blue, "Blue")];
            widgets::segmented(ui, ("curve-channel", lid), &mut ed.channel, &options, true);
            ui.add_space(10.0);
            let color = match ed.channel {
                Channel::Rgb => theme::FG,
                Channel::Red => Color32::from_rgb(0xff, 0x6b, 0x6b),
                Channel::Green => Color32::from_rgb(0x5c, 0xe0, 0x8a),
                Channel::Blue => Color32::from_rgb(0x6b, 0x9b, 0xff),
            };
            curve_editor(ui, id.with(("curve", lid, format!("{:?}", ed.channel))), c.points_mut(ed.channel), color, &histogram);
            ui.add_space(8.0);
            paragraph(ui, "Drag a point; press on the curve to add one; double-click a point or drag it out to remove it.");
            ui.add_space(8.0);
            let b = Button::new("Straighten this curve").icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).id_salt(("curve-reset", lid));
            let bw = b.desired_width(ui);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                *c.points_mut(ed.channel) = vec![[0.0, 0.0], [1.0, 1.0]];
            }
        }
        Effect::Vignette(v) => {
            w::label(ui, "Vignette");
            w::slider(ui, ("amount", lid), "Amount", &mut v.amount, -1.0..=1.0, 0.6, w::signed);
            w::slider(ui, ("size", lid), "Size", &mut v.size, 0.3..=2.0, 1.0, w::percent);
            w::slider(ui, ("feather", lid), "Softness", &mut v.feather, 0.0..=1.0, 0.5, w::percent);
            ui.add_space(4.0);
            paragraph(ui, "Below zero it lightens the edges. Drag its centre on the picture.");
            center_button(ui, ("vig-centre", lid), &mut v.center);
        }
        Effect::Aberration(a) => {
            w::label(ui, "Chromatic aberration");
            w::slider(ui, ("amount", lid), "Amount", &mut a.amount, 0.0..=3.0, 1.0, |v| format!("{v:.2}"));
            paragraph(ui, "Red and blue part away from the centre, more towards the edges. Drag the centre on the picture.");
            center_button(ui, ("ab-centre", lid), &mut a.center);
        }
        Effect::Comic(c) => {
            w::label(ui, "Comic");
            let mut tones = c.tones as f32;
            if w::slider(ui, ("tones", lid), "Tones", &mut tones, photo::Comic::MIN_TONES as f32..=photo::Comic::MAX_TONES as f32, 4.0, |v| format!("{}", v.round() as u32)) {
                c.tones = tones.round() as u32;
            }
            w::slider(ui, ("ink", lid), "Ink lines", &mut c.ink, 0.0..=1.0, 0.6, w::percent);
            w::switch(ui, ("dots", lid), "Print dots", &mut c.dots);
            if c.dots {
                w::slider(ui, ("dot-size", lid), "Dot size", &mut c.dot_size, 0.0..=1.0, 0.3, w::percent);
            }
            ui.add_space(4.0);
            paragraph(ui, "The Toolkit photoshoot's comic look. On a VRChat layer (the people), its outline is inked too.");
        }
        Effect::Blur(b) => {
            ui.add_space(8.0);
            let options = [SegOption::new(BlurKind::Gaussian, "Gaussian"), SegOption::new(BlurKind::Lens, "Lens"), SegOption::new(BlurKind::Directional, "Directional")];
            widgets::segmented(ui, ("blur-kind", lid), &mut b.kind, &options, true);
            ui.add_space(8.0);
            let max = match b.kind {
                BlurKind::Gaussian => render::GAUSSIAN_MAX,
                BlurKind::Lens => render::LENS_MAX,
                BlurKind::Directional => render::DIRECTIONAL_MAX,
            };
            b.radius = b.radius.min(max);
            let label = match b.kind {
                BlurKind::Directional => "Length",
                _ => "Radius",
            };
            w::slider(ui, ("radius", lid), label, &mut b.radius, 0.0..=max, 0.01, |v| format!("{:.1} %", v * 100.0));
            match b.kind {
                BlurKind::Lens => {
                    w::slider(ui, ("bokeh", lid), "Bokeh", &mut b.bokeh, 0.0..=1.0, 0.5, w::percent);
                }
                BlurKind::Directional => {
                    w::slider(ui, ("angle", lid), "Angle", &mut b.angle, -90.0..=90.0, 0.0, |v| format!("{v:+.0}°"));
                }
                BlurKind::Gaussian => {}
            }
            ui.add_space(4.0);
            paragraph(ui, "Keep part of the picture sharp with Where: a circle (inverted) or a band, like a tilt-shift.");
        }
        Effect::Text(t) => {
            w::label(ui, "Words");
            let edit = egui::TextEdit::multiline(&mut t.value)
                .id(id.with(("text", lid)))
                .desired_rows(2)
                .desired_width(ui.available_width())
                .font(theme::font(14.0, weight::MEDIUM))
                .text_color(theme::FG)
                .margin(egui::Margin::symmetric(10, 8))
                .hint_text(egui::RichText::new("Type something…").color(theme::FG_4));
            egui::Frame::NONE.fill(theme::INK_750).corner_radius(theme::radius(12.0)).show(ui, |ui| {
                ui.add(edit);
            });
            w::slider(ui, ("text-size", lid), "Size", &mut t.size, photo::Text::MIN_SIZE..=photo::Text::MAX_SIZE, 0.13, |v| format!("{:.0} %", v * 100.0));
            w::label(ui, "Style");
            let options: Vec<SegOption<TextStyle>> = TextStyle::ALL.iter().map(|s| SegOption::new(*s, s.name())).collect();
            widgets::segmented(ui, ("text-style", lid), &mut t.style, &options, true);
            w::label(ui, "Colour");
            const COLORS: [[u8; 3]; 12] = [
                [255, 255, 255],
                [16, 16, 20],
                [0, 218, 109],
                [72, 230, 255],
                [116, 102, 255],
                [189, 101, 255],
                [255, 107, 122],
                [255, 77, 109],
                [245, 183, 79],
                [255, 230, 90],
                [255, 160, 200],
                [140, 140, 150],
            ];
            if let Some(c) = w::swatches(ui, id.with(("swatches", lid)), &COLORS, t.color) {
                t.color = c;
            }
            w::label(ui, "Font");
            ed.fonts.ensure(&ctx);
            font_list(&mut ed.fonts, ui, id.with(("fonts", lid)), &mut t.font);
            ui.add_space(6.0);
            paragraph(ui, "Drag the text on the picture to move it.");
        }
    }
    if kind != Kind::Text {
        mask(ui, lid, &mut l.mask);
    }
}

/// Where a layer acts, and the shape's sliders.
fn mask(ui: &mut Ui, lid: u32, m: &mut Mask) {
    w::label(ui, "Where");
    #[derive(Clone, Copy, PartialEq)]
    enum Shape {
        All,
        Circle,
        Band,
    }
    let mut shape = match m {
        Mask::Everywhere => Shape::All,
        Mask::Radial { .. } => Shape::Circle,
        Mask::Band { .. } => Shape::Band,
    };
    let options = [SegOption::new(Shape::All, "Everywhere"), SegOption::new(Shape::Circle, "Circle"), SegOption::new(Shape::Band, "Band")];
    if widgets::segmented(ui, ("mask", lid), &mut shape, &options, true) {
        let center = m.center().unwrap_or([0.5, 0.5]);
        *m = match shape {
            Shape::All => Mask::Everywhere,
            Shape::Circle => Mask::radial(),
            Shape::Band => Mask::band(),
        };
        m.set_center(center);
    }
    ui.add_space(6.0);
    match m {
        Mask::Everywhere => {}
        Mask::Radial { radius, feather, invert, .. } => {
            w::slider(ui, ("mask-radius", lid), "Radius", radius, 0.02..=1.5, 0.25, w::percent);
            w::slider(ui, ("mask-feather", lid), "Softness", feather, 0.0..=1.0, 0.2, w::percent);
            w::switch(ui, ("mask-invert", lid), "Outside the circle", invert);
        }
        Mask::Band { angle, width, feather, invert, .. } => {
            w::slider(ui, ("mask-width", lid), "Width", width, 0.02..=1.5, 0.2, w::percent);
            w::slider(ui, ("mask-feather", lid), "Softness", feather, 0.0..=1.0, 0.15, w::percent);
            w::slider(ui, ("mask-angle", lid), "Angle", angle, -90.0..=90.0, 0.0, |v| format!("{v:+.0}°"));
            w::switch(ui, ("mask-invert", lid), "Outside the band", invert);
        }
    }
}

fn center_button(ui: &mut Ui, salt: impl egui::AsIdSalt, center: &mut [f32; 2]) {
    ui.add_space(8.0);
    let b = Button::new("Centre it").icon(icons::CROSSHAIR).size(Size::Sm).enabled(*center != [0.5, 0.5]).id_salt(salt);
    let bw = b.desired_width(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
        *center = [0.5, 0.5];
    }
}

/// A searchable list of fonts, each name drawn in its font; the chosen one ringed.
fn font_list(fonts: &mut Fonts, ui: &mut Ui, id: Id, chosen: &mut String) {
    let w = ui.available_width();
    widgets::search_field(ui, id.with("search"), &mut fonts.query, "Search fonts", w);
    ui.add_space(8.0);
    let q = fonts.query.trim().to_lowercase();
    let shown: Vec<FontInfo> = fonts.list.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).cloned().collect();
    let loading = fonts.installed.as_ref().is_some_and(|j| j.running());
    let ctx = ui.ctx().clone();
    let row_h = 40.0;
    egui::Frame::NONE.fill(theme::INK_850).corner_radius(theme::radius(12.0)).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(id.with("scroll")).max_height(260.0).auto_shrink([false, true]).show_rows(ui, row_h, shown.len(), |ui, range| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let mut made = 0;
            for f in &shown[range] {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                let rid = id.with(("font", &f.key));
                let resp = ui.interact(r, rid, Sense::click());
                let selected = *chosen == f.key;
                let hover = orbiters_ui::anim::toggle(&ctx, rid.with("hover"), resp.hovered(), orbiters_ui::anim::Spring::SNAPPY);
                let p = ui.painter();
                p.rect_filled(r.shrink(1.0), theme::radius(10.0), if selected { theme::alpha(theme::ACCENT, 0.14) } else { theme::white(0.04 * hover) });
                // Made a few a frame, so scrolling never waits on many at once.
                let preview = if fonts.previews.contains_key(&f.key) || made < 4 {
                    made += 1;
                    fonts.preview(&ctx, f)
                } else {
                    ctx.request_repaint();
                    None
                };
                let max_w = r.width() - 20.0;
                match preview {
                    Some(t) => {
                        let s = t.size_vec2() / ctx.pixels_per_point();
                        let k = (max_w / s.x).min((row_h - 10.0) / s.y).min(1.0);
                        let size = s * k;
                        let at = Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - size.y * 0.5), size);
                        p.image(t.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if selected { theme::ACCENT } else { theme::FG });
                    }
                    None => {
                        widgets::text_truncated(p, pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(&f.name, 13.0), max_w);
                    }
                }
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(if f.bundled { format!("{} (built in)", f.name) } else { f.name.clone() });
                if widgets::pressed(&resp) {
                    *chosen = f.key.clone();
                }
            }
        });
    });
    if loading {
        ui.add_space(6.0);
        paragraph(ui, "Looking for this computer's fonts…");
    }
}

/// A curve over the picture's histogram: its points dragged, added by a press on the curve, removed by a double-click
/// or by dragging them out.
fn curve_editor(ui: &mut Ui, id: Id, points: &mut Vec<[f32; 2]>, color: Color32, histogram: &[u32]) {
    let side = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(r.expand(8.0).intersect(ui.clip_rect()));
    p.rect_filled(r, theme::radius(12.0), theme::INK_850);
    let box_ = r.shrink(10.0);
    let to_screen = |q: [f32; 2]| pos2(box_.left() + q[0] * box_.width(), box_.bottom() - q[1] * box_.height());
    let from_screen = |s: egui::Pos2| [((s.x - box_.left()) / box_.width()).clamp(0.0, 1.0), ((box_.bottom() - s.y) / box_.height()).clamp(0.0, 1.0)];
    // The histogram, then a grid and the straight line.
    let most = histogram.iter().copied().max().unwrap_or(0).max(1) as f32;
    for (i, n) in histogram.iter().enumerate() {
        let x0 = box_.left() + box_.width() * i as f32 / histogram.len() as f32;
        let x1 = box_.left() + box_.width() * (i + 1) as f32 / histogram.len() as f32;
        let h = box_.height() * (*n as f32 / most).sqrt() * 0.9;
        p.rect_filled(Rect::from_min_max(pos2(x0, box_.bottom() - h), pos2(x1, box_.bottom())), 0.0, theme::white(0.06));
    }
    for k in 1..4 {
        let t = k as f32 / 4.0;
        p.vline(box_.left() + box_.width() * t, box_.y_range(), egui::Stroke::new(1.0, theme::white(0.05)));
        p.hline(box_.x_range(), box_.top() + box_.height() * t, egui::Stroke::new(1.0, theme::white(0.05)));
    }
    p.line_segment([to_screen([0.0, 0.0]), to_screen([1.0, 1.0])], egui::Stroke::new(1.0, theme::white(0.12)));
    // Dragging a point: which one.
    let drag_id = id.with("dragging");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id)).flatten();
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    let nearest = |pts: &[[f32; 2]], s: egui::Pos2| pts.iter().enumerate().map(|(i, q)| (i, to_screen(*q).distance(s))).filter(|(_, d)| *d < 12.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    if resp.double_clicked() {
        if let Some(i) = pointer.and_then(|s| nearest(points, s)).filter(|i| *i > 0 && *i + 1 < points.len()) {
            points.remove(i);
        }
        dragging = None;
    } else if widgets::pressed(&resp) {
        if let Some(s) = pointer {
            dragging = nearest(points, s).or_else(|| {
                let q = from_screen(s);
                let i = points.iter().position(|p| p[0] > q[0]).unwrap_or(points.len());
                points.insert(i, q);
                Some(i)
            });
        }
    }
    if let (Some(i), Some(s)) = (dragging, resp.interact_pointer_pos()) {
        if resp.dragged() && i < points.len() {
            let end = i == 0 || i + 1 == points.len();
            let out = s.y < r.top() - 24.0 || s.y > r.bottom() + 24.0;
            if out && !end {
                points.remove(i);
                dragging = None;
            } else {
                let q = from_screen(s);
                let lo = if i == 0 { 0.0 } else { points[i - 1][0] + 0.01 };
                let hi = if i + 1 == points.len() { 1.0 } else { points[i + 1][0] - 0.01 };
                let x = if i == 0 && points.len() > 1 { q[0].min(hi) } else if i + 1 == points.len() { q[0].max(lo) } else { q[0].clamp(lo, hi.max(lo)) };
                points[i] = [x, q[1]];
            }
        }
    }
    if !resp.is_pointer_button_down_on() && !resp.dragged() {
        dragging = None;
    }
    ui.data_mut(|d| d.insert_temp(drag_id, dragging));
    // The curve and its points.
    let line: Vec<egui::Pos2> = (0..=96).map(|k| {
        let x = k as f32 / 96.0;
        to_screen([x, photo::tone::curve(points, x)])
    }).collect();
    p.add(egui::Shape::line(line, egui::Stroke::new(2.0, color)));
    for (i, q) in points.iter().enumerate() {
        let c = to_screen(*q);
        let active = dragging == Some(i);
        p.circle_filled(c, if active { 6.5 } else { 5.0 }, color);
        p.circle_stroke(c, if active { 6.5 } else { 5.0 }, egui::Stroke::new(1.5, theme::INK_900));
    }
    resp.on_hover_cursor(CursorIcon::Crosshair);
}
