//! How Photos & captures lays a group's pictures out ([`GalleryLayout`], its picker over the gallery): Masonry, the
//! website's galleries' columns (`server/frontend/src/components/elements/GalleryMasonry.jsx`: as many columns as fit at
//! most 240 points wide, 16 apart, each picture in the shortest column, as tall as its shape); Rows, pictures side by
//! side as wide as their shape, each row as tall as fills the width (the website's Even rows); Grid, the same card for
//! every item (its picture cropped to 16:9, what it is and who sees it under it). A picture's shape is its size from
//! the listing, else its thumbnail's once decoded (springing there from 4:3, the website's stand-in, never jumping).
use egui::emath::GuiRounding as _;
use egui::{pos2, vec2, Id, Rect, Sense, Ui};
use std::ops::Range;
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
    grid_shaped(n, width, min, 16.0 / 9.0, body)
}

/// [`grid`] with pictures of shape `aspect` (width over height): VRChat's avatar thumbnails are 4:3.
pub fn grid_shaped(n: usize, width: f32, min: f32, aspect: f32, body: f32) -> (Vec<Rect>, f32) {
    let (cols, w) = orbiters_ui::layout::columns(width, min, GAP);
    let h = w / aspect + body;
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

/// What the places of a list's pictures come from: the same key, the same places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// The list's revision, its groups and their headings with it.
    pub entries: u64,
    /// The revision of the pictures' shapes (0 where the layout asks for none).
    pub aspects: u64,
    pub layout: GalleryLayout,
    pub width: f32,
    /// Between what is stacked in the list (egui's item spacing).
    pub spacing: f32,
}

/// A group in the list: its entries, its heading's top and height, where it ends and its tallest picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub entries: Range<usize>,
    pub top: f32,
    pub head: f32,
    pub bottom: f32,
    tallest: f32,
}

impl Group {
    /// Its heading, in a list `width` wide, from the list's top left.
    pub fn heading(&self, width: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, self.top), vec2(width, self.head))
    }
}

/// Where the list's groups and pictures are, from the list's top left: made once for a [`Key`] and looked up at every
/// frame by what is in view. (Every group was laid out twice a frame, with a key cloned for each picture.)
pub struct Places {
    pub key: Key,
    /// Different for each one made.
    pub made: u64,
    pub groups: Vec<Group>,
    /// Each entry's, by its place in the list. Their tops never go up along the list: groups are stacked, and a layout
    /// puts each picture no higher than the one before.
    pub cells: Vec<Rect>,
    /// The list's, its last gap with it.
    pub height: f32,
}

/// Between a group's heading and its pictures, and after them.
const UNDER_HEADING: f32 = 12.0;
const UNDER_GROUP: f32 = 32.0;

impl Places {
    /// `groups` of entries in the list's order, each under a heading as tall as `head` says of its first entry;
    /// `aspect`: a picture's shape.
    pub fn make(key: Key, made: u64, groups: &[Range<usize>], head: impl Fn(usize) -> f32, aspect: impl Fn(usize) -> f32, card_min: f32, card_body: f32) -> Self {
        let mut out = Places { key, made, groups: Vec::with_capacity(groups.len()), cells: Vec::with_capacity(groups.last().map_or(0, |g| g.end)), height: 0.0 };
        let mut aspects = Vec::new();
        let mut y = 0.0f32;
        for entries in groups {
            let (top, head) = (y.round_ui(), head(entries.start));
            aspects.clear();
            if tiles(key.layout) {
                aspects.extend(entries.clone().map(&aspect));
            } else {
                aspects.resize(entries.len(), 0.0);
            }
            let (cells, height) = arrange(key.layout, &aspects, key.width, card_min, card_body);
            let area = (top + head + key.spacing + UNDER_HEADING).round_ui();
            let tallest = cells.iter().map(Rect::height).fold(0.0, f32::max);
            out.cells.extend(cells.into_iter().map(|c| c.translate(vec2(0.0, area))));
            out.groups.push(Group { entries: entries.clone(), top, head, bottom: area + height, tallest });
            y = area + height + key.spacing + UNDER_GROUP;
        }
        out.height = y;
        out
    }

    /// The groups with something between `from` and `to`.
    pub fn groups_within(&self, from: f32, to: f32) -> &[Group] {
        let first = self.groups.partition_point(|g| g.bottom < from);
        let end = self.groups.partition_point(|g| g.top <= to);
        &self.groups[first.min(end)..end]
    }

    /// The entries of `group` that may show between `from` and `to`.
    pub fn cells_within(&self, group: &Group, from: f32, to: f32) -> Range<usize> {
        let cells = &self.cells[group.entries.clone()];
        let first = cells.partition_point(|c| c.top() + group.tallest < from);
        let end = cells.partition_point(|c| c.top() <= to);
        group.entries.start + first.min(end)..group.entries.start + end
    }

    /// The topmost entry from `y` down and its top; of those at the same height, the one `key` orders first.
    pub fn topmost<'k>(&self, y: f32, key: impl Fn(usize) -> &'k str) -> Option<(usize, f32)> {
        let first = self.cells.partition_point(|c| c.top() < y);
        let top = self.cells.get(first)?.top();
        let same = self.cells[first..].iter().take_while(|c| c.top() == top).count();
        (first..first + same).min_by(|a, b| key(*a).cmp(key(*b))).map(|i| (i, top))
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
        p.rect_filled(Rect::from_min_size(pos2(track.left() + x, track.top() + 4.0), vec2(36.0, 32.0)), theme::radius(16.0), theme::ink_600());
    }
    for (i, (value, glyph, _)) in options.iter().enumerate() {
        let target = if *value == *layout {
            theme::fg()
        } else if hovered[i] {
            theme::fg_2()
        } else {
            theme::fg_3()
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
