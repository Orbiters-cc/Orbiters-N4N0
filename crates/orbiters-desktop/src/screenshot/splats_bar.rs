//! `--only splats-bar` (night3, verifier point 7): the 3D view's top row on narrow bars, with three recorded views,
//! in English and French, a short world name and a long one, a capture of this computer (four buttons) and a download
//! (two). The real widgets (the title chip, Save this view, Edit in 3D, Select splats, Export… or their icons, Recorded
//! view, the views' steps and "View n of m", Close) are placed by the view's own layout functions
//! (`pages::splats::{title_room, action_row}`) over a dark stand-in for the splats: the view itself needs a
//! reconstructed capture and a GPU, which this run does not have. What it shows is the layout; the viewer draws the same
//! widgets at the same rectangles.

use crate::pages::splats::{action_row, title_room, view_controls_left, RESET_W, STEP_LABEL_W, STEP_W};
use egui::{pos2, vec2, Align2, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_i18n::{mark, tr};
use orbiters_ui::offscreen::save_png;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Variant};
use orbiters_ui::icons;
use std::path::Path;

pub(super) fn shots(out: &Path) -> anyhow::Result<()> {
    for language in [orbiters_i18n::Language::En, orbiters_i18n::Language::Fr] {
        for (width, actions) in [(808.0f32, 4usize), (808.0, 2), (1008.0, 4), (1208.0, 4)] {
            for (name, title) in [("short", "VRChat Home"), ("long", "Beginner's Unity Workshop: your first world")] {
                let image = orbiters_i18n::in_language(language, || paint(width, actions, title));
                let (w, h) = (width as u32, 76u32);
                let path = out.join(format!("splats-bar-{}-{}-{}-{name}.png", language.tag(), width as u32, if actions == 4 { "capture" } else { "download" }));
                save_png(&path, &super::over_ground(&image, w, h))?;
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

/// One bar, `width` points wide (the view's), three recorded views, `actions` buttons.
fn paint(width: f32, actions: usize, title: &str) -> Vec<u8> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let size = vec2(width, 76.0);
    let mut sw = crate::overlay::painter();
    let mut image = Vec::new();
    for f in 0..3 {
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(f as f64 / 60.0), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
        let mut full = ctx.run_ui(input, |ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let p = ui.painter().clone();
            // A dark scene for the splats.
            p.rect_filled(rect, 0.0, theme::mix(theme::INK_950, theme::ACCENT, 0.08));
            let bar = Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(rect.width() - 32.0, 44.0));
            let views = 3;
            let chip = |r: Rect| p.rect_filled(r, theme::radius(r.height() * 0.5), theme::black(0.55));
            let title_t = Txt::new(title, 14.0).weight(weight::SEMIBOLD);
            let sub = orbiters_i18n::tr!("{count} splats", count = orbiters_ui::format::integer(731_469.0));
            let sub_t = Txt::new(sub, 11.0).color(theme::FG_3);
            let title_w = (widgets::text_size(ui, &title_t).x.max(widgets::text_size(ui, &sub_t).x) + 40.0).min(title_room(bar, views, actions));
            let title_r = Rect::from_min_size(bar.min, vec2(title_w, 44.0));
            chip(title_r);
            widgets::text_truncated(&p, pos2(title_r.left() + 20.0, title_r.center().y - 7.0), Align2::LEFT_CENTER, &title_t, title_w - 40.0);
            widgets::text_truncated(&p, pos2(title_r.left() + 20.0, title_r.center().y + 9.0), Align2::LEFT_CENTER, &sub_t, title_w - 40.0);
            let buttons = [
                (tr("Save this view"), icons::CAMERA, mark("Save this view")),
                (tr("Edit in 3D"), icons::SLIDERS_HORIZONTAL, mark("Edit in 3D")),
                (tr("Select splats"), icons::CROSSHAIR, mark("Select splats")),
                (tr("Export…"), icons::UPLOAD_SIMPLE, mark("Export…")),
            ];
            let natural = buttons.map(|(label, glyph, _)| Button::new(label).icon(glyph).variant(Variant::Glass).desired_width(ui));
            let (rects, icon) = action_row(title_r, view_controls_left(bar.right(), views), natural, actions);
            for i in 0..actions {
                let (label, glyph, english) = buttons[i];
                if icon[i] {
                    IconButton::new(glyph, english).size(40.0, 17.0).backdrop(theme::black(0.55)).id_salt(("bar-icon", i)).show_at(ui, rects[i]);
                } else {
                    Button::new(label).icon(glyph).variant(Variant::Glass).id_salt(("bar", i)).show_at(ui, rects[i]);
                }
            }
            // The view's controls on the right, as the viewer places them.
            let close_r = Rect::from_min_size(pos2(bar.right() - 44.0, bar.top()), vec2(44.0, 44.0));
            IconButton::new(icons::X, mark("Close the 3D view")).size(44.0, 18.0).backdrop(theme::black(0.55)).show_at(ui, close_r);
            let reset_r = Rect::from_min_size(pos2(close_r.left() - 10.0 - RESET_W, bar.top() + 2.0), vec2(RESET_W, 40.0));
            Button::new(tr("Recorded view")).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Glass).width(RESET_W).show_at(ui, reset_r);
            let next_r = Rect::from_min_size(pos2(reset_r.left() - 10.0 - STEP_W, bar.top() + 2.0), vec2(STEP_W, 40.0));
            let label_r = Rect::from_min_size(pos2(next_r.left() - STEP_LABEL_W, bar.top() + 2.0), vec2(STEP_LABEL_W, 40.0));
            let prev_r = Rect::from_min_size(pos2(label_r.left() - STEP_W, bar.top() + 2.0), vec2(STEP_W, 40.0));
            chip(Rect::from_min_max(prev_r.min, next_r.max));
            widgets::text(&p, label_r.center(), Align2::CENTER_CENTER, &Txt::new(orbiters_i18n::tr!("View {n} of {total}", n = 2, total = 3), 12.0).color(theme::FG_2));
            Button::new("‹").variant(Variant::Ghost).width(STEP_W).show_at(ui, prev_r);
            Button::new("›").variant(Variant::Ghost).width(STEP_W).show_at(ui, next_r);
        });
        sw.set_textures(&mut full.textures_delta);
        let prims = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        image = sw.paint(&prims, full.pixels_per_point, theme::INK_950).to_vec();
        sw.free_textures(&mut full.textures_delta);
    }
    image
}
