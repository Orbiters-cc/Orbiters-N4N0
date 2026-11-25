//! The Blender files an avatar or a world was linked to with Mixer or MCB ([`orbiters_projects::blender`]), under its
//! row: each with the tool that linked it, when it was last saved, and Open in Blender (the newest Blender with
//! Orbiters' add-on, else the newest, else what .blend files open with).

use orbiters_i18n::tr;
use super::art;
use super::state::{Outcome, State};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui};
use orbiters_projects::{avatars::Avatar, blender};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use orbiters_ui::icons;

/// Blender's orange, on its files' tiles.
const BLENDER: egui::Color32 = egui::Color32::from_rgb(234, 118, 0);

/// The rows of `target`'s Blender files (none without any), `indent` points in from the left.
pub(super) fn rows(ui: &mut Ui, state: &mut State, target: &Avatar, indent: f32) {
    let links = links(state, target);
    if links.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    // Indented only where the names keep room enough.
    let button_w = Button::new(tr("Open in Blender")).icon(icons::BLENDER).size(Size::Sm).desired_width(ui);
    let indent = if width - indent - 48.0 - button_w < 180.0 { 0.0 } else { indent };
    let (head, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    widgets::text(ui.painter(), pos2(head.left() + indent, head.center().y + 2.0), Align2::LEFT_CENTER, &Txt::caps(tr("Blender files")));
    let now = orbiters_projects::discovery::now();
    for (k, l) in links.iter().enumerate() {
        let (row, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
        let row = Rect::from_min_max(pos2(row.left() + indent, row.top()), row.max);
        let tile = Rect::from_min_size(pos2(row.left(), row.center().y - 15.0), vec2(30.0, 30.0));
        art::tile(ui.painter(), tile, icons::BLENDER, BLENDER);
        let b = Button::new(tr("Open in Blender"))
            .icon(icons::BLENDER)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(!state.running())
            .id_salt(("open-blend", &target.id, &l.blend));
        let bw = b.desired_width(ui);
        let br = Rect::from_min_size(pos2(row.right() - bw, row.center().y - 16.0), vec2(bw, 32.0));
        let x = tile.right() + 10.0;
        let tw = br.left() - 8.0 - x;
        let name = l.blend.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let p = ui.painter();
        widgets::text_truncated(p, pos2(x, row.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(&name, 13.0).weight(weight::MEDIUM), tw);
        let detail = tr!("{tool} · saved {when}", tool = l.tool.label(), when = crate::pages::ago(ui.ctx(), l.modified, now));
        widgets::text_truncated(p, pos2(x, row.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(detail, 11.5).color(theme::fg_3()), tw);
        // Where it is, whole, on hover.
        let _ = ui
            .interact(Rect::from_min_max(pos2(tile.left(), row.top()), pos2(br.left() - 4.0, row.bottom())), Id::unique(("blend-row", &target.id, &l.blend)), Sense::hover())
            .on_hover_text(l.blend.display().to_string());
        state.anchors.insert(format!("open-blend-{k}"), br);
        if widgets::activated(&b.show_at(ui, br)) {
            open(&ctx, state, l);
        }
    }
}

/// The Blender files linked to `target`, newest first.
pub(super) fn links(state: &State, target: &Avatar) -> Vec<blender::Link> {
    blender::linked(&state.links, target).into_iter().cloned().collect()
}

/// Opens `link`'s file in Blender (on a worker; a toast says in which).
pub(super) fn open(ctx: &egui::Context, state: &mut State, link: &blender::Link) {
    let blend = link.blend.clone();
    let name = blend.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.task(ctx, tr("Opening Blender…"), move || {
        let by = blender::open(&blend)?;
        Ok(Outcome::Said(tr!("{name} opened in {program}", name = name, program = by)))
    });
}
