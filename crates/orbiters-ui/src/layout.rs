//! Layout maths of the pages, apart from painting so that it can be tested: a side panel that sits beside the content
//! on wide windows and slides over it on narrow ones, a toolbar that fits beside a heading or goes under it, card
//! grids, and columns of section cards on wide pages.

/// Page widths (points, the sidebar excluded) from which a side panel sits beside the content instead of over it. The
/// window's default size (1280 points, a 1048-point page) docks it.
pub const DOCK_FROM: f32 = 980.0;
/// The narrowest the content gets beside a docked panel (two cards of a grid).
pub const CONTENT_MIN: f32 = 520.0;
/// Bounds of a docked panel's width.
pub const PANEL_MIN: f32 = 360.0;
pub const PANEL_MAX: f32 = 620.0;
/// Space between the content and a docked panel.
pub const PANE_GAP: f32 = 20.0;

/// How a page shares its width with a side panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    /// Beside the content (true) or sliding over it.
    pub docked: bool,
    /// The panel's width.
    pub panel: f32,
    /// The content's width inside the page's padding: all of it under a sliding panel, less the panel and the gap
    /// beside a docked one.
    pub content: f32,
}

/// The widest a docked panel gets on a page `width` wide with `pad` of padding on each side: the content keeps
/// [`CONTENT_MIN`].
pub fn panel_max(width: f32, pad: f32) -> f32 {
    (width - 2.0 * pad - PANE_GAP - CONTENT_MIN).min(PANEL_MAX).max(PANEL_MIN)
}

/// How a page `width` wide with `pad` of padding on each side splits with a side panel; `wanted` is the width the
/// person dragged a docked panel to.
pub fn split(width: f32, pad: f32, wanted: Option<f32>) -> Split {
    let inner = (width - 2.0 * pad).max(0.0);
    if width < DOCK_FROM {
        let panel = (width * 0.72).clamp(420.0, 640.0).min(width - 24.0).max(0.0);
        return Split { docked: false, panel, content: inner };
    }
    let panel = wanted.unwrap_or((width * 0.36).clamp(380.0, 480.0)).clamp(PANEL_MIN, panel_max(width, pad));
    Split { docked: true, panel, content: (inner - panel - PANE_GAP).max(0.0) }
}

/// How many cards of at least `min` points, `gap` apart, fit in `width`, and how wide each then is.
pub fn columns(width: f32, min: f32, gap: f32) -> (usize, f32) {
    let n = (((width + gap) / (min + gap)).floor() as usize).max(1);
    (n, ((width - gap * (n - 1) as f32) / n as f32).max(0.0))
}

/// The narrowest a column of section cards (Settings', N4N0's) gets beside another one.
pub const SECTION_MIN: f32 = 600.0;
/// The widest a section card gets: alone on a page, and side by side.
pub const SECTION_ALONE_MAX: f32 = 1120.0;
pub const SECTION_MAX: f32 = 980.0;
/// The most columns of section cards.
pub const SECTION_COLUMNS: usize = 3;

/// How many columns of section cards a page's content `width` wide gets, `gap` apart, and how wide each is: one as wide
/// as the page (up to [`SECTION_ALONE_MAX`]) until two of [`SECTION_MIN`] fit, then side by side.
pub fn section_columns(width: f32, gap: f32) -> (usize, f32) {
    let n = (((width + gap) / (SECTION_MIN + gap)).floor() as usize).clamp(1, SECTION_COLUMNS);
    let w = ((width - gap * (n - 1) as f32) / n as f32).max(0.0);
    (n, w.min(if n == 1 { SECTION_ALONE_MAX } else { SECTION_MAX }))
}

/// Widths for items that each want `wanted` points on a row `total` wide (`gap` between them): their wishes scaled to
/// fill the row exactly, so that the wordier ones keep more room.
pub fn share(wanted: &[f32], total: f32, gap: f32) -> Vec<f32> {
    let room = (total - gap * wanted.len().saturating_sub(1) as f32).max(0.0);
    let sum: f32 = wanted.iter().map(|w| w.max(1.0)).sum();
    wanted.iter().map(|w| w.max(1.0) * room / sum.max(1.0)).collect()
}

/// How much of what items want a row of them gets (below 1: they are cut).
pub fn share_scale(wanted: &[f32], total: f32, gap: f32) -> f32 {
    let room = (total - gap * wanted.len().saturating_sub(1) as f32).max(0.0);
    room / wanted.iter().map(|w| w.max(1.0)).sum::<f32>().max(1.0)
}

/// Where a toolbar goes on a heading's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Beside the heading, every action with its label.
    Beside,
    /// Beside the heading, the secondary actions as round icon buttons.
    Compact,
    /// On its own row under the heading, with its labels.
    Below,
}

/// Space kept between a heading's text and the toolbar beside it.
pub const TOOLBAR_GAP: f32 = 24.0;

/// Where a toolbar `full` points wide with its labels (`compact` without) goes on a row `row` wide whose heading needs
/// `title` points.
pub fn toolbar(row: f32, title: f32, full: f32, compact: f32) -> Fit {
    if title + TOOLBAR_GAP + full <= row {
        Fit::Beside
    } else if title + TOOLBAR_GAP + compact <= row {
        Fit::Compact
    } else {
        Fit::Below
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_docks_on_the_default_window_and_slides_over_narrower_ones() {
        // The default window: 1280 points less the 232-point sidebar.
        let s = split(1048.0, 32.0, None);
        assert!(s.docked);
        assert_eq!(s.panel, 380.0);
        assert_eq!(s.content, 1048.0 - 64.0 - 380.0 - PANE_GAP);
        assert!(s.content >= CONTENT_MIN);
        assert_eq!(columns(s.content, 250.0, 16.0).0, 2);
        // The narrowest window (1040 points): over the content, as wide as it can be without hiding all of it.
        let s = split(808.0, 32.0, None);
        assert!(!s.docked);
        assert_eq!(s.content, 808.0 - 64.0);
        assert!(s.panel >= 420.0 && s.panel <= 808.0 - 24.0);
        assert!(!split(DOCK_FROM - 0.5, 32.0, None).docked && split(DOCK_FROM, 32.0, None).docked);
    }

    #[test]
    fn a_wide_page_gives_the_panel_more_but_never_starves_the_content() {
        // A maximised 1920-point window.
        let s = split(1688.0, 32.0, None);
        assert_eq!(s.panel, 480.0);
        assert_eq!(columns(s.content, 250.0, 16.0).0, 4);
        // Dragged wider than the page allows: the content keeps its minimum.
        let s = split(1048.0, 32.0, Some(900.0));
        assert_eq!(s.content, CONTENT_MIN);
        assert_eq!(s.panel, panel_max(1048.0, 32.0));
        // Dragged narrower than the minimum, or wider than the maximum on a huge page.
        assert_eq!(split(1688.0, 32.0, Some(100.0)).panel, PANEL_MIN);
        assert_eq!(split(3000.0, 32.0, Some(2000.0)).panel, PANEL_MAX);
        // A page just wide enough to dock: the panel at its minimum, the content at or above its own.
        let s = split(DOCK_FROM, 32.0, Some(PANEL_MAX));
        assert!(s.panel >= PANEL_MIN && s.content >= CONTENT_MIN - 0.01, "{s:?}");
    }

    #[test]
    fn columns_fill_the_width() {
        assert_eq!(columns(984.0, 250.0, 16.0), (3, (984.0 - 32.0) / 3.0));
        assert_eq!(columns(200.0, 250.0, 16.0), (1, 200.0));
        let (n, w) = columns(1124.0, 250.0, 16.0);
        assert!((n as f32 * w + (n - 1) as f32 * 16.0 - 1124.0).abs() < 0.01);
    }

    #[test]
    fn section_cards_go_side_by_side_on_wide_pages() {
        // Pages (less their padding) of 1100, 1280, 1600, 1920, 2560 and 3440-point windows.
        assert_eq!(section_columns(804.0, 24.0), (1, 804.0));
        assert_eq!(section_columns(984.0, 24.0), (1, 984.0));
        assert_eq!(section_columns(1304.0, 24.0), (2, 640.0));
        assert_eq!(section_columns(1624.0, 24.0), (2, 800.0));
        let (n, w) = section_columns(2264.0, 24.0);
        assert!(n == 3 && (w - (2264.0 - 48.0) / 3.0).abs() < 0.01);
        assert_eq!(section_columns(3144.0, 24.0), (3, SECTION_MAX));
        // Alone, never wider than readable.
        assert_eq!(section_columns(1200.0, 24.0), (1, SECTION_ALONE_MAX));
    }

    #[test]
    fn shares_fill_the_row_in_proportion() {
        let w = share(&[100.0, 200.0, 100.0], 436.0, 18.0);
        assert!((w.iter().sum::<f32>() + 36.0 - 436.0).abs() < 0.01);
        assert!((w[1] - 2.0 * w[0]).abs() < 0.01 && w[0] == w[2]);
        assert!((share_scale(&[100.0, 200.0, 100.0], 436.0, 18.0) - 1.0).abs() < 0.01);
        assert!(share_scale(&[300.0, 300.0], 312.0, 12.0) < 0.51);
    }

    #[test]
    fn a_toolbar_drops_its_labels_before_its_row() {
        assert_eq!(toolbar(984.0, 300.0, 600.0, 420.0), Fit::Beside);
        assert_eq!(toolbar(800.0, 300.0, 600.0, 420.0), Fit::Compact);
        assert_eq!(toolbar(600.0, 300.0, 600.0, 420.0), Fit::Below);
    }
}
