//! How Photos & captures lays a group's pictures out ([`GalleryLayout`], its picker over the gallery): Masonry, the
//! website's galleries' columns (`server/frontend/src/components/elements/GalleryMasonry.jsx`: as many columns as fit at
//! most 240 points wide, 16 apart, each picture in the shortest column, as tall as its shape); Rows, pictures side by
//! side as wide as their shape, each row as tall as fills the width (the website's Even rows); Grid, the same card for
//! every item (its picture cropped to 16:9, what it is and who sees it under it). A picture's shape is its size from
//! the listing, else its thumbnail's once decoded (springing there from 4:3, the website's stand-in, never jumping).
use egui::{pos2, vec2, Id, Rect, Sense, Ui};
use orbiters_core::settings::GalleryLayout;
use orbiters_i18n::{mark, tr};
use orbiters_ui::{
    anim::{self, Spring},
    icons,
    theme,
    widgets,
};

/// Space between pictures, as the website's.
pub const GAP: f32 = 16.0;
/// Masonry's columns are at most this wide (the website's 240).
pub const COLUMN_MAX: f32 = 240.0;
/// A row's height before it is fitted to the width.
pub const ROW_HEIGHT: f32 = 210.0;
/// The shape of a picture whose size is not known yet (the website's 800 × 600).
pub const STAND_IN: f32 = 4.0 / 3.0;
/// Shapes are kept within these (a panorama or a sliver still gets a usable card).
const SHAPES: (f32, f32) = (0.3, 4.0);

fn shape(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 {
        aspect.clamp(SHAPES.0, SHAPES.1)
    } else {
        STAND_IN
    }
}

/// The website's masonry: `ceil((width + gap) / (240 + gap))` columns, each picture (width over height `aspects`) in the
/// shortest one, the leftmost of equals. Rects from the group's top left, and the group's height.
pub fn masonry(aspects: &[f32], width: f32) -> (Vec<Rect>, f32) {
    let columns = (((width + GAP) / (COLUMN_MAX + GAP)).ceil() as usize).max(1);
    let w = ((width - GAP * (columns - 1) as f32) / columns as f32).max(1.0);
    let mut bottoms = vec![0.0f32; columns];
    let rects = aspects
        .iter()
        .map(|a| {
            let (column, top) = bottoms.iter().copied().enumerate().fold((0, f32::INFINITY), |m, (i, b)| if b < m.1 { (i, b) } else { m });
            let h = w / shape(*a);
            bottoms[column] = top + h + GAP;
            Rect::from_min_size(pos2(column as f32 * (w + GAP), top), vec2(w, h))
        })
        .collect();
    let height = bottoms.iter().copied().fold(0.0, f32::max) - if aspects.is_empty() { 0.0 } else { GAP };
    (rects, height.max(0.0))
}

/// Justified rows: pictures side by side at [`ROW_HEIGHT`] until the row is full, then the row scaled to the width
/// exactly (ending it before the picture that overfills it when that keeps it nearer the height); the last row keeps
/// the height, from the left.
pub fn rows(aspects: &[f32], width: f32) -> (Vec<Rect>, f32) {
    let mut rects = Vec::with_capacity(aspects.len());
    let mut y = 0.0;
    let mut row: Vec<f32> = Vec::new();
    let fill = |row: &[f32]| (width - GAP * (row.len().saturating_sub(1)) as f32).max(1.0) / row.iter().sum::<f32>();
    let place = |row: &[f32], h: f32, y: &mut f32, rects: &mut Vec<Rect>| {
        let mut x = 0.0;
        for a in row {
            let w = a * h;
            rects.push(Rect::from_min_size(pos2(x, *y), vec2(w, h)));
            x += w + GAP;
        }
        *y += h + GAP;
    };
    for a in aspects.iter().map(|a| shape(*a)) {
        row.push(a);
        let natural = row.iter().sum::<f32>() * ROW_HEIGHT + GAP * (row.len() - 1) as f32;
        if natural < width {
            continue;
        }
        let with = fill(&row);
        let without = (row.len() > 1).then(|| fill(&row[..row.len() - 1]));
        match without.filter(|h| (h - ROW_HEIGHT).abs() < (with - ROW_HEIGHT).abs()) {
            Some(h) => {
                place(&row[..row.len() - 1], h, &mut y, &mut rects);
                row = vec![a];
            }
            None => {
                place(&row, with, &mut y, &mut rects);
                row.clear();
            }
        }
    }
    if !row.is_empty() {
        // The last row as tall as the others are meant to be, unless that overfills it.
        let h = ROW_HEIGHT.min(fill(&row));
        place(&row, h, &mut y, &mut rects);
    }
    (rects, (y - if aspects.is_empty() { 0.0 } else { GAP }).max(0.0))
}

/// Grid: cards of `card_h` (their picture and body) in as many columns of at least `min` as fit.
pub fn grid(n: usize, width: f32, min: f32, body: f32) -> (Vec<Rect>, f32) {
    let (cols, w) = orbiters_ui::layout::columns(width, min, GAP);
    let h = w * 9.0 / 16.0 + body;
    let rects = (0..n).map(|i| Rect::from_min_size(pos2((i % cols) as f32 * (w + GAP), (i / cols) as f32 * (h + GAP)), vec2(w, h))).collect();
    (rects, (n.div_ceil(cols) as f32 * (h + GAP) - GAP).max(0.0))
}

/// Whether the layout shows pictures alone (their shape kept, what they are on them), not cards.
pub fn tiles(layout: GalleryLayout) -> bool {
    !matches!(layout, GalleryLayout::Grid)
}

/// A group laid out: each picture's rect from its top left, and its height.
pub fn arrange(layout: GalleryLayout, aspects: &[f32], width: f32, card_min: f32, card_body: f32) -> (Vec<Rect>, f32) {
    match layout {
        GalleryLayout::Masonry => masonry(aspects, width),
        GalleryLayout::Rows => rows(aspects, width),
        GalleryLayout::Grid => grid(aspects.len(), width, card_min, card_body),
    }
}

/// The picker's width.
pub const PICKER_W: f32 = 3.0 * 36.0 + 8.0;

/// The layout picker: three icons on a track, the chosen one's pill sliding between them (as [`widgets::segmented`]).
/// Returns true when the layout changed (on press).
pub fn picker(ui: &mut Ui, rect: Rect, layout: &mut GalleryLayout) -> bool {
    let id = Id::unique("gallery-layout-picker");
    let p = ui.painter().clone();
    let track = Rect::from_center_size(rect.center(), vec2(PICKER_W, 40.0));
    p.rect_filled(track, theme::radius(20.0), theme::surface_soft());
    let options = [
        (GalleryLayout::Masonry, icons::COLUMNS, mark("Masonry: columns of pictures, each as tall as its shape")),
        (GalleryLayout::Rows, icons::ROWS, mark("Rows: pictures side by side, each row filling the width")),
        (GalleryLayout::Grid, icons::SQUARES_FOUR, mark("Grid: the same card for every item, with what it is and who sees it")),
    ];
    let rects: Vec<Rect> = (0..options.len()).map(|i| Rect::from_min_size(pos2(track.left() + 4.0 + i as f32 * 36.0, track.top() + 4.0), vec2(36.0, 32.0))).collect();
    let mut changed = false;
    let mut hovered = [false; 3];
    for (i, (value, _, hint)) in options.iter().enumerate() {
        let resp = ui.interact(rects[i], id.with(i), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        hovered[i] = resp.hovered();
        if widgets::activated(&resp) && *layout != *value {
            *layout = *value;
            changed = true;
        }
        resp.on_hover_text(tr(hint));
    }
    if let Some(active) = options.iter().position(|o| o.0 == *layout) {
        let [x] = anim::spring(ui.ctx(), id.with("pill"), [rects[active].left() - track.left()], Spring::BOUNCY);
        let x = x.clamp(4.0, track.width() - 40.0);
        p.rect_filled(Rect::from_min_size(pos2(track.left() + x, track.top() + 4.0), vec2(36.0, 32.0)), theme::radius(16.0), theme::INK_600);
    }
    for (i, (value, glyph, _)) in options.iter().enumerate() {
        let target = if *value == *layout {
            theme::FG
        } else if hovered[i] {
            theme::FG_2
        } else {
            theme::FG_3
        };
        let color = anim::color(ui.ctx(), id.with(("c", i)), target, Spring::SNAPPY);
        widgets::icon(&p, rects[i].center(), glyph, 16.0, color);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.left() < b.right() - 0.01 && a.right() > b.left() + 0.01 && a.top() < b.bottom() - 0.01 && a.bottom() > b.top() + 0.01
    }

    /// The website's test: columns at most 240 wide, nothing overlapping or out of the width, and pictures added later
    /// never moving those placed before (a next page appends).
    #[test]
    fn masonry_columns_match_the_website_and_appending_keeps_places() {
        let aspects: Vec<f32> = (0..120).map(|i| [800.0 / 600.0, 800.0 / 1200.0, 800.0 / 4000.0, 4.0][i % 4]).collect();
        for width in [334.0, 964.0, 1600.0] {
            let (first, _) = masonry(&aspects[..24], width);
            let (all, height) = masonry(&aspects, width);
            assert_eq!(&all[..24], &first[..]);
            for (i, a) in all.iter().enumerate() {
                assert!(a.left() >= 0.0 && a.right() <= width + 0.01 && a.bottom() <= height + 0.01);
                assert!(a.width() <= COLUMN_MAX + 0.01);
                assert!(all[i + 1..].iter().all(|b| !overlaps(a, b)), "{width}: {i} overlaps");
            }
        }
        // 964 points: ceil(980 / 256) = 4 columns of 229, the first four pictures across the top.
        let (r, _) = masonry(&[1.0; 5], 964.0);
        assert_eq!(r[3].left(), 3.0 * (229.0 + GAP));
        assert_eq!((r[4].left(), r[4].top()), (0.0, 229.0 + GAP));
    }

    #[test]
    fn a_picture_keeps_its_shape_in_every_tile_layout() {
        let aspects = [16.0 / 9.0, 9.0 / 16.0, 1.0, 21.0 / 9.0, 4.0 / 3.0, 3.0 / 2.0, 1.0];
        for layout in [GalleryLayout::Masonry, GalleryLayout::Rows] {
            let (rects, _) = arrange(layout, &aspects, 1000.0, 270.0, 70.0);
            for (r, a) in rects.iter().zip(aspects) {
                assert!((r.width() / r.height() - a).abs() < 0.01, "{layout:?}: {} for {a}", r.width() / r.height());
            }
        }
    }

    #[test]
    fn rows_fill_the_width_and_the_last_row_keeps_its_height() {
        let aspects = [16.0 / 9.0; 7];
        let (rects, height) = rows(&aspects, 1000.0);
        // 16:9 at 210 points is 373 wide: two fit, the third overfills, so rows of three are fitted down.
        let first: Vec<&Rect> = rects.iter().filter(|r| r.top() == 0.0).collect();
        assert_eq!(first.len(), 3);
        assert!((first.last().unwrap().right() - 1000.0).abs() < 0.01);
        assert!(first[0].height() < ROW_HEIGHT);
        let last = rects.last().unwrap();
        assert_eq!(last.height(), ROW_HEIGHT);
        assert_eq!(last.left(), 0.0);
        assert!((height - last.bottom()).abs() < 0.01);
        assert!(rects.iter().enumerate().all(|(i, a)| rects[i + 1..].iter().all(|b| !overlaps(a, b))));
    }

    #[test]
    fn unknown_shapes_stand_in_as_four_by_three() {
        let (r, _) = masonry(&[f32::NAN, 0.0], 240.0);
        assert!((r[0].width() / r[0].height() - STAND_IN).abs() < 0.01);
        assert!((r[1].width() / r[1].height() - STAND_IN).abs() < 0.01);
    }

    #[test]
    fn the_grid_is_the_cards_it_was() {
        let (rects, height) = grid(5, 1000.0, 270.0, 70.0);
        let (cols, w) = orbiters_ui::layout::columns(1000.0, 270.0, 16.0);
        assert_eq!(cols, 3);
        assert_eq!(rects[3].min, pos2(0.0, w * 9.0 / 16.0 + 70.0 + 16.0));
        assert!((height - (2.0 * (w * 9.0 / 16.0 + 70.0) + 16.0)).abs() < 0.01);
    }
}
