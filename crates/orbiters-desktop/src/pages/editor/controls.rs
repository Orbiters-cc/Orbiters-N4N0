//! The chosen layer's controls on the editor's right: its opacity and where it acts (everywhere, in a circle or along
//! a band), then its own: light's and colour's sliders, a curve to drag per channel over the picture's histogram, the
//! vignette's and the aberration's strength, the comic's tones, ink and dots, the blur's kind and size, the text's
//! words, font (the built-in ones, then this computer's, each shown in itself), size, style and colour. Crop and rotate
//! has its shapes, straighten, quarter turns and flips.

use super::{glyph, layers, tint, widgets as w, Editor, Selected};
use crate::pages::Job;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_i18n::{mark, tr};
use orbiters_ui::photo::text::FontInfo;
use orbiters_ui::photo::{self, render, Aspect, Blur, BlurKind, Channel, Effect, Focus, Kind, Mask, TextStyle};
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
    if ed.live.is_some() {
        header(ui, icons::SLIDERS_HORIZONTAL, theme::ACCENT, tr("Edit in 3D"), tr("The layers go on every frame while you move around. Save picture writes this view and its edited copy, which opens again here."));
    } else {
        header(ui, icons::SLIDERS_HORIZONTAL, theme::ACCENT, tr("Edit a copy"), tr("The picture itself is never changed: saving writes a copy beside it, with its edit, which opens again here."));
    }
    let size = ed.loaded.as_ref().map(|l| tr!("{width} × {height} pixels", width = l.full.0, height = l.full.1)).unwrap_or_else(|| tr("Reading the picture…").to_string());
    paragraph(ui, &size);
    if !ed.doc.sources.is_empty() {
        ui.add_space(8.0);
        let names: Vec<String> = ed.doc.sources.iter().rev().map(|s| layers::source_name(&s.name)).collect();
        paragraph(ui, &tr!("A multi-layer picture from VRChat's camera: {names}. Each has its own effects, under the whole picture's.", names = layers::list(&names)));
    }
    w::label(ui, tr("Start with"));
    let quick = [Kind::Light, Kind::Color, Kind::Comic, Kind::Text];
    let widths: Vec<f32> = quick.iter().map(|k| Chip::new(k.name()).icon(glyph(*k)).width(ui)).collect();
    let (pos, h) = widgets::flow(&widths, ui.available_width(), widgets::CHIP_H, 8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let title = ed.title.clone();
    for ((k, p), cw) in quick.iter().zip(pos).zip(widths) {
        let cr = Rect::from_min_size(r.min + p, vec2(cw, widgets::CHIP_H));
        if Chip::new(k.name()).icon(glyph(*k)).id_salt(("quick", glyph(*k))).show_at(ui, cr).pressed {
            layers::add(ed, *k, &title);
        }
    }
    ui.add_space(14.0);
    paragraph(
        ui,
        if cfg!(target_os = "macos") {
            tr("Cmd+Z undoes, Cmd+Shift+Z redoes, Delete removes the chosen layer. Hold the eye at the top to see the picture as it was.")
        } else {
            tr("Ctrl+Z undoes, Ctrl+Shift+Z redoes, Delete removes the chosen layer. Hold the eye at the top to see the picture as it was.")
        },
    );
}

fn crop(ed: &mut Editor, ui: &mut Ui) {
    header(
        ui,
        icons::CROP,
        theme::ACCENT,
        tr("Crop and rotate"),
        if cfg!(target_os = "macos") {
            tr("Drag the frame's corners or edges, or the picture under it; a pinch or two fingers zoom. The dials under the picture straighten it and set its perspective upright; AUTO levels it by its own lines.")
        } else {
            tr("Drag the frame's corners or edges, or the picture under it; the wheel zooms. The dials under the picture straighten it and set its perspective upright; AUTO levels it by its own lines.")
        },
    );
    let full = ed.loaded.as_ref().map_or((16, 9), |l| l.full);
    let g = &mut ed.doc.geometry;
    let (tw, th) = photo::geometry::turned_size(full.0, full.1, g.turns);
    let (tw, th) = (tw as f32, th as f32);
    let original = tw / th.max(1.0);
    w::label(ui, tr("Shape"));
    let mut landscape = super::crop::landscape(&ed.crop, g, original);
    let options = [SegOption::new(false, tr("Portrait")), SegOption::new(true, tr("Landscape"))];
    if widgets::segmented(ui, "crop-orientation", &mut landscape, &options, true) {
        super::crop::set_landscape(&mut ed.crop, g, landscape, tw, th);
    }
    ui.add_space(10.0);
    let list: Vec<Aspect> = Aspect::PRESETS.iter().map(|a| a.oriented(landscape, original)).collect();
    let widths: Vec<f32> = list.iter().map(|a| Chip::new(a.name()).selected(g.aspect.preset() == a.preset()).width(ui)).collect();
    let (pos, h) = widgets::flow(&widths, ui.available_width(), widgets::CHIP_H, 8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    for (n, ((a, p), cw)) in list.iter().zip(pos).zip(widths).enumerate() {
        let cr = Rect::from_min_size(r.min + p, vec2(cw, widgets::CHIP_H));
        if Chip::new(a.name()).selected(g.aspect.preset() == a.preset()).id_salt(("aspect", n)).show_at(ui, cr).pressed {
            super::crop::set_aspect(g, *a, tw, th);
        }
    }
    w::label(ui, tr("Straighten and perspective"));
    for dial in super::crop::Dial::ALL {
        if w::slider(ui, ("crop-dial", dial.name()), tr(dial.name()), dial.value_mut(g), -45.0..=45.0, 0.0, |v| format!("{v:+.1}°")) {
            ed.crop.dial = dial;
        }
    }
    ui.add_space(6.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let buttons = [
        (icons::ARROW_ARC_LEFT, mark("Rotate a quarter to the left")),
        (icons::ARROW_ARC_RIGHT, mark("Rotate a quarter to the right")),
        (icons::FLIP_HORIZONTAL, mark("Flip across")),
        (icons::FLIP_VERTICAL, mark("Flip upside down")),
    ];
    for (i, (glyph, label)) in buttons.iter().enumerate() {
        let r = Rect::from_min_size(pos2(row.left() + i as f32 * 48.0, row.top()), vec2(40.0, 40.0));
        if widgets::activated(&IconButton::new(glyph, tr(label)).size(40.0, 18.0).backdrop(theme::white(0.05)).id_salt(("crop-turn", i)).show_at(ui, r)) {
            match i {
                0 | 1 => super::crop::turn(g, i == 0),
                2 => super::crop::flip(g, true),
                _ => super::crop::flip(g, false),
            }
        }
    }
    ui.add_space(12.0);
    let b = Button::new(tr("Reset")).icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).enabled(!g.is_identity() || g.aspect != Aspect::Free).id_salt("crop-reset");
    let bw = b.desired_width(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
        *g = photo::Geometry::default();
    }
    ui.add_space(12.0);
    let (ow, oh) = g.output_size(full.0, full.1);
    paragraph(ui, &tr!("The copy will be {width} × {height} pixels.", width = ow, height = oh));
}

fn source(ed: &mut Editor, ui: &mut Ui, i: usize) {
    let Some(s) = ed.doc.sources.get_mut(i) else { return };
    let name = layers::source_name(&s.name);
    let about = match s.name.to_ascii_lowercase().as_str() {
        "environment" => tr("The world, as VRChat's camera saved it apart from the people."),
        "player" | "players" | "users" => tr("The people, transparent around them: blur the world and keep them sharp, or give them the comic look alone."),
        "ui" => tr("The interface VRChat drew over the picture, transparent elsewhere."),
        _ => tr("The picture VRChat saved with its layers, all of them in one."),
    };
    header(ui, layers::source_glyph(&s.name), theme::FG_2, &name, about);
    w::switch(ui, ("source-visible", i), tr("Shown"), &mut s.visible);
    ui.add_space(4.0);
    w::slider(ui, ("source-opacity", i), tr("Opacity"), &mut s.opacity, 0.0..=1.0, 1.0, w::percent);
    ui.add_space(12.0);
    let b = Button::new(tr("Add an effect to it")).icon(icons::PLUS).size(Size::Sm).id_salt(("source-add", i));
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
    let acts = [(icons::ARROW_UP, mark("Move up")), (icons::ARROW_DOWN, mark("Move down")), (icons::COPY, mark("Duplicate")), (icons::TRASH, mark("Remove"))];
    for (i, (glyph, label)) in acts.iter().enumerate() {
        let r = Rect::from_min_size(pos2(row.left() + i as f32 * 44.0, row.top()), vec2(36.0, 36.0));
        let mut b = IconButton::new(glyph, tr(label)).size(36.0, 16.0).backdrop(theme::white(0.05)).id_salt(("layer-act", i));
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
    w::slider(ui, ("opacity", lid), tr("Opacity"), &mut l.opacity, 0.0..=1.0, 1.0, w::percent);
    match &mut l.effect {
        Effect::Light(v) => {
            w::label(ui, tr("Light"));
            w::slider_look(ui, ("exposure", lid), tr("Exposure"), &mut v.exposure, -4.0..=4.0, 0.0, |v| format!("{v:+.2} EV"), w::Look::Exposure);
            w::slider_look(ui, ("brightness", lid), tr("Brightness"), &mut v.brightness, -1.0..=1.0, 0.0, w::signed, w::Look::Brightness);
            w::slider_look(ui, ("contrast", lid), tr("Contrast"), &mut v.contrast, -1.0..=1.0, 0.0, w::signed, w::Look::Contrast);
            w::slider_look(ui, ("highlights", lid), tr("Highlights"), &mut v.highlights, -1.0..=1.0, 0.0, w::signed, w::Look::Highlights);
            w::slider_look(ui, ("shadows", lid), tr("Shadows"), &mut v.shadows, -1.0..=1.0, 0.0, w::signed, w::Look::Shadows);
            w::slider_look(ui, ("black", lid), tr("Black point"), &mut v.black_point, -0.25..=0.5, 0.0, w::signed, w::Look::BlackPoint);
        }
        Effect::Color(v) => {
            w::label(ui, tr("White balance"));
            w::slider_look(ui, ("temperature", lid), tr("Temperature"), &mut v.temperature, -1.0..=1.0, 0.0, w::signed, w::Look::Temperature);
            w::slider_look(ui, ("tint", lid), tr("Tint"), &mut v.tint, -1.0..=1.0, 0.0, w::signed, w::Look::Tint);
            w::label(ui, tr("Colour"));
            w::slider_look(ui, ("saturation", lid), tr("Saturation"), &mut v.saturation, -1.0..=1.0, 0.0, w::signed, w::Look::Saturation);
            w::slider_look(ui, ("vibrance", lid), tr("Vibrance"), &mut v.vibrance, -1.0..=1.0, 0.0, w::signed, w::Look::Vibrance);
            w::slider_look(ui, ("hue", lid), tr("Hue"), &mut v.hue, -180.0..=180.0, 0.0, |v| format!("{v:+.0}°"), w::Look::Hue);
        }
        Effect::Curves(c) => {
            ui.add_space(8.0);
            let options = [SegOption::new(Channel::Rgb, tr("RGB")), SegOption::new(Channel::Red, tr("Red")), SegOption::new(Channel::Green, tr("Green")), SegOption::new(Channel::Blue, tr("Blue"))];
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
            paragraph(ui, tr("Drag a point; press on the curve to add one; double-click a point or drag it out to remove it."));
            ui.add_space(8.0);
            let b = Button::new(tr("Straighten this curve")).icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).id_salt(("curve-reset", lid));
            let bw = b.desired_width(ui);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                *c.points_mut(ed.channel) = vec![[0.0, 0.0], [1.0, 1.0]];
            }
        }
        Effect::Vignette(v) => {
            w::label(ui, tr("Vignette"));
            w::slider_look(ui, ("amount", lid), tr("Amount"), &mut v.amount, -1.0..=1.0, 0.6, w::signed, w::Look::Vignette);
            w::slider(ui, ("size", lid), tr("Size"), &mut v.size, 0.3..=2.0, 1.0, w::percent);
            w::slider(ui, ("feather", lid), tr("Softness"), &mut v.feather, 0.0..=1.0, 0.5, w::percent);
            ui.add_space(4.0);
            paragraph(ui, tr("Below zero it lightens the edges. Drag its centre on the picture."));
            center_button(ui, ("vig-centre", lid), &mut v.center);
        }
        Effect::Aberration(a) => {
            w::label(ui, tr("Chromatic aberration"));
            w::slider_look(ui, ("amount", lid), tr("Amount"), &mut a.amount, 0.0..=3.0, 1.0, |v| format!("{v:.2}"), w::Look::Aberration);
            paragraph(ui, tr("Red and blue part away from the centre, more towards the edges. Drag the centre on the picture."));
            center_button(ui, ("ab-centre", lid), &mut a.center);
        }
        Effect::Comic(c) => {
            w::label(ui, tr("Comic"));
            let mut tones = c.tones as f32;
            if w::slider(ui, ("tones", lid), tr("Tones"), &mut tones, photo::Comic::MIN_TONES as f32..=photo::Comic::MAX_TONES as f32, 4.0, |v| format!("{}", v.round() as u32)) {
                c.tones = tones.round() as u32;
            }
            w::slider_look(ui, ("ink", lid), tr("Ink lines"), &mut c.ink, 0.0..=1.0, 0.6, w::percent, w::Look::Ink);
            w::switch(ui, ("dots", lid), tr("Print dots"), &mut c.dots);
            if c.dots {
                w::slider(ui, ("dot-size", lid), tr("Dot size"), &mut c.dot_size, 0.0..=1.0, 0.3, w::percent);
            }
            ui.add_space(4.0);
            paragraph(ui, tr("The Toolkit photoshoot's comic look. On a VRChat layer (the people), its outline is inked too."));
        }
        Effect::Blur(b) => {
            // By depth: on a 3D view's splats (Edit in 3D, or an edit saved there).
            if ed.live.is_some() || b.kind == BlurKind::Depth {
                let mut by_depth = b.kind == BlurKind::Depth;
                if w::switch(ui, ("blur-depth", lid), tr("Depth of field"), &mut by_depth) {
                    b.kind = if by_depth { BlurKind::Depth } else { BlurKind::Gaussian };
                }
            }
            if b.kind == BlurKind::Depth {
                ui.add_space(4.0);
                depth(ui, lid, b, ed.live.as_mut());
                return;
            }
            ui.add_space(8.0);
            let options = [SegOption::new(BlurKind::Gaussian, tr("Gaussian")), SegOption::new(BlurKind::Lens, tr("Lens")), SegOption::new(BlurKind::Directional, tr("Directional"))];
            widgets::segmented(ui, ("blur-kind", lid), &mut b.kind, &options, true);
            ui.add_space(8.0);
            let max = match b.kind {
                BlurKind::Gaussian => render::GAUSSIAN_MAX,
                BlurKind::Lens => render::LENS_MAX,
                BlurKind::Directional => render::DIRECTIONAL_MAX,
                BlurKind::Depth => 0.0,
            };
            b.radius = b.radius.min(max);
            let label = match b.kind {
                BlurKind::Directional => tr("Length"),
                _ => tr("Radius"),
            };
            w::slider(ui, ("radius", lid), label, &mut b.radius, 0.0..=max, 0.01, |v| format!("{:.1} %", v * 100.0));
            match b.kind {
                BlurKind::Lens => {
                    w::slider(ui, ("bokeh", lid), tr("Bokeh"), &mut b.bokeh, 0.0..=1.0, 0.5, w::percent);
                }
                BlurKind::Directional => {
                    w::slider(ui, ("angle", lid), tr("Angle"), &mut b.angle, -90.0..=90.0, 0.0, |v| format!("{v:+.0}°"));
                }
                BlurKind::Gaussian | BlurKind::Depth => {}
            }
            ui.add_space(4.0);
            paragraph(ui, tr("Keep part of the picture sharp with Where: a circle (inverted) or a band, like a tilt-shift."));
        }
        Effect::Text(t) => {
            w::label(ui, tr("Words"));
            let edit = egui::TextEdit::multiline(&mut t.value)
                .id(id.with(("text", lid)))
                .desired_rows(2)
                .desired_width(ui.available_width())
                .font(theme::font(14.0, weight::MEDIUM))
                .text_color(theme::FG)
                .margin(egui::Margin::symmetric(10, 8))
                .hint_text(egui::RichText::new(tr("Type something…")).color(theme::FG_4));
            egui::Frame::NONE.fill(theme::INK_750).corner_radius(theme::radius(12.0)).show(ui, |ui| {
                ui.add(edit);
            });
            w::slider(ui, ("text-size", lid), tr("Size"), &mut t.size, photo::Text::MIN_SIZE..=photo::Text::MAX_SIZE, 0.13, |v| format!("{:.0} %", v * 100.0));
            w::label(ui, tr("Style"));
            let options: Vec<SegOption<TextStyle>> = TextStyle::ALL.iter().map(|s| SegOption::new(*s, s.name())).collect();
            widgets::segmented(ui, ("text-style", lid), &mut t.style, &options, true);
            w::label(ui, tr("Colour"));
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
            w::label(ui, tr("Font"));
            ed.fonts.ensure(&ctx);
            font_list(&mut ed.fonts, ui, id.with(("fonts", lid)), &mut t.font);
            ui.add_space(6.0);
            paragraph(ui, tr("Drag the text on the picture to move it."));
        }
    }
    if kind != Kind::Text {
        mask(ui, lid, &mut l.mask);
    }
}

/// A distance in metres, as the depth blur shows it.
pub(super) fn distance_text(m: f32) -> String {
    if m >= 0.9 * orbiters_reconstruct::dof::FAR_FOCUS {
        tr("far away").to_string()
    } else if m < 1.0 {
        format!("{:.0} cm", m * 100.0)
    } else if m < 10.0 {
        format!("{m:.1} m")
    } else {
        format!("{m:.0} m")
    }
}

/// A depth blur's controls: how the focus is found (the dot on the picture, or a distance), the aperture and how much
/// shows in front of the focus and behind it. Drawn on the splats (Edit in 3D): it has no Where.
fn depth(ui: &mut Ui, lid: u32, b: &mut Blur, mut live: Option<&mut super::Live>) {
    if live.is_none() {
        paragraph(ui, tr("Depth of field blurs a 3D view's splats by how far they are from the focus: it shows and saves in Edit in 3D. A picture has no depth, so here it changes nothing."));
        ui.add_space(6.0);
    }
    w::label(ui, tr("Focus"));
    let options = [SegOption::new(Focus::Auto, Focus::Auto.name()), SegOption::new(Focus::Manual, Focus::Manual.name())];
    widgets::segmented(ui, ("dof-focus", lid), &mut b.focus, &options, true);
    ui.add_space(6.0);
    let measured = live.as_ref().and_then(|l| l.focus);
    let mut setting = false;
    match b.focus {
        Focus::Auto => {
            paragraph(ui, tr("Drag the focus dot on the picture: what lies under it stays sharp, and the focus follows it as you move around."));
            if let Some(f) = measured {
                ui.add_space(4.0);
                paragraph(ui, &tr!("Focused at {distance}.", distance = distance_text(f)));
            }
            ui.add_space(8.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            let centre = Button::new(tr("Centre it")).icon(icons::CROSSHAIR).size(Size::Sm).enabled(b.point != [0.5, 0.5]).id_salt(("dof-centre", lid));
            let cw = centre.desired_width(ui);
            if super::super::sharing::pressed(centre.show_at(ui, Rect::from_min_size(row.min, vec2(cw, 32.0)))) {
                b.point = [0.5, 0.5];
            }
            if let Some(f) = measured.filter(|f| *f < 0.9 * orbiters_reconstruct::dof::FAR_FOCUS) {
                let keep = Button::new(tr("Keep this distance")).size(Size::Sm).id_salt(("dof-keep", lid));
                let kw = keep.desired_width(ui);
                if super::super::sharing::pressed(keep.show_at(ui, Rect::from_min_size(row.min + vec2(cw + 8.0, 0.0), vec2(kw, 32.0)))) {
                    b.focus = Focus::Manual;
                    b.distance = f.clamp(Blur::MIN_DISTANCE, Blur::MAX_DISTANCE);
                }
            }
        }
        Focus::Manual => {
            // Along the log of the distance: as fine near the eye as far from it.
            let (lo, hi) = (Blur::MIN_DISTANCE.ln(), Blur::MAX_DISTANCE.ln());
            let mut t = b.distance.clamp(Blur::MIN_DISTANCE, Blur::MAX_DISTANCE).ln();
            if w::slider(ui, ("dof-distance", lid), tr("Distance"), &mut t, lo..=hi, 3f32.ln(), |t| distance_text(t.exp())) {
                b.distance = t.exp();
            }
            setting = w::slider_held(ui, ("dof-distance", lid));
            if let Some(l) = live.as_deref_mut() {
                w::switch(ui, ("dof-plane", lid), tr("Show the focus while setting it"), &mut l.show_plane);
                paragraph(ui, tr("While you drag the distance, what lies at it is tinted green."));
            }
        }
    }
    if let Some(l) = live {
        l.setting_focus = setting;
    }
    w::label(ui, tr("Lens"));
    w::slider(ui, ("dof-aperture", lid), tr("Aperture"), &mut b.aperture, 0.0..=1.0, 0.4, w::percent);
    w::slider(ui, ("dof-near", lid), tr("Foreground"), &mut b.near, 0.0..=1.0, 1.0, w::percent);
    w::slider(ui, ("dof-far", lid), tr("Background"), &mut b.far, 0.0..=1.0, 1.0, w::percent);
    ui.add_space(4.0);
    paragraph(ui, tr("Each splat spreads over a lens's disc as wide as it is out of focus, in front of the focus and behind it. Opacity lowers the aperture."));
}

/// Where a layer acts, and the shape's sliders.
fn mask(ui: &mut Ui, lid: u32, m: &mut Mask) {
    w::label(ui, tr("Where"));
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
    let options = [SegOption::new(Shape::All, tr("Everywhere")), SegOption::new(Shape::Circle, tr("Circle")), SegOption::new(Shape::Band, tr("Band"))];
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
            w::slider(ui, ("mask-radius", lid), tr("Radius"), radius, 0.02..=1.5, 0.25, w::percent);
            w::slider(ui, ("mask-feather", lid), tr("Softness"), feather, 0.0..=1.0, 0.2, w::percent);
            w::switch(ui, ("mask-invert", lid), tr("Outside the circle"), invert);
        }
        Mask::Band { angle, width, feather, invert, .. } => {
            w::slider(ui, ("mask-width", lid), tr("Width"), width, 0.02..=1.5, 0.2, w::percent);
            w::slider(ui, ("mask-feather", lid), tr("Softness"), feather, 0.0..=1.0, 0.15, w::percent);
            w::slider(ui, ("mask-angle", lid), tr("Angle"), angle, -90.0..=90.0, 0.0, |v| format!("{v:+.0}°"));
            w::switch(ui, ("mask-invert", lid), tr("Outside the band"), invert);
        }
    }
}

fn center_button(ui: &mut Ui, salt: impl egui::AsIdSalt, center: &mut [f32; 2]) {
    ui.add_space(8.0);
    let b = Button::new(tr("Centre it")).icon(icons::CROSSHAIR).size(Size::Sm).enabled(*center != [0.5, 0.5]).id_salt(salt);
    let bw = b.desired_width(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
        *center = [0.5, 0.5];
    }
}

/// A searchable list of fonts, each name drawn in its font; the chosen one ringed.
fn font_list(fonts: &mut Fonts, ui: &mut Ui, id: Id, chosen: &mut String) {
    let w = ui.available_width();
    widgets::search_field(ui, id.with("search"), &mut fonts.query, tr("Search fonts"), w);
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
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(if f.bundled { tr!("{font} (built in)", font = f.name) } else { f.name.clone() });
                if widgets::pressed(&resp) {
                    *chosen = f.key.clone();
                }
            }
        });
    });
    if loading {
        ui.add_space(6.0);
        paragraph(ui, tr("Looking for this computer's fonts…"));
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
    // The tones in and out along its sides, black to the channel's colour.
    let ramp = |r: Rect, across: bool| {
        let mut mesh = egui::Mesh::default();
        let (a, b) = (Color32::BLACK, color);
        if across {
            mesh.colored_vertex(r.left_top(), a);
            mesh.colored_vertex(r.left_bottom(), a);
            mesh.colored_vertex(r.right_top(), b);
            mesh.colored_vertex(r.right_bottom(), b);
        } else {
            mesh.colored_vertex(r.left_bottom(), a);
            mesh.colored_vertex(r.right_bottom(), a);
            mesh.colored_vertex(r.left_top(), b);
            mesh.colored_vertex(r.right_top(), b);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        p.add(egui::Shape::mesh(mesh));
    };
    ramp(Rect::from_min_max(pos2(box_.left(), box_.bottom() + 3.0), pos2(box_.right(), box_.bottom() + 7.0)), true);
    ramp(Rect::from_min_max(pos2(box_.left() - 7.0, box_.top()), pos2(box_.left() - 3.0, box_.bottom())), false);
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
