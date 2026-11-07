//! The FPS booster's sheet, from All tools: four large cells across as the slider, not a thin track, each comfortably more than
//! what can be pressed at arm's length. Under them the step's one line, the one status line that matters, and before and after.
//! Its words are `crate::booster`'s, the window's too, and it draws from the same state.
//!
//! Retro needs nothing of its own: the pass over what is drawn gives it capitals, cut corners and the ramp. In one colour the
//! chosen cell is told by being filled, and the steps it includes by being partly filled.

use super::*;
use crate::booster;
use crate::state::Snapshot;
use orbiters_booster::guided::Guide;
use orbiters_booster::trial::{Comparison, Kind};
use orbiters_booster::Level;

/// What can be pressed on this sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoosterPick {
    /// A stop of the slider: pressed or dragged across.
    Level(Level),
    Details(bool),
    Test(Kind),
    PutBack,
    OpenLogs,
    Done,
    Next,
    Previous,
    /// "Show me how", the first time on Avatars.
    Understood,
    StayOnPicture,
    PutBackDone,
    HoldTest(bool),
    Crash,
    TestLoad(bool),
    Report,
}

/// What the sheet draws, made from the snapshot each time, so that it is in the language now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoosterFacts {
    pub level: Level,
    /// The cell a press is held on, seen before it is let go.
    pub held: Option<Level>,
    pub details: bool,
    pub asking: bool,
    pub told: bool,
    pub status: String,
    pub now: Option<String>,
    pub comparison: Option<Comparison>,
    pub streamer: bool,
    pub guide: Guide,
    pub item_now: String,
    pub changed: Vec<String>,
    pub times: Option<String>,
    pub to_put_back: Vec<String>,
    pub panel_3d: bool,
    pub reconstruction: bool,
    pub test_tools: bool,
    pub test_load: bool,
    pub holding: bool,
}

impl BoosterFacts {
    pub fn of(snap: &Snapshot, details: bool, asking: bool, held: Option<Level>) -> Self {
        let status = &snap.booster;
        let report = &status.overlay.report;
        Self {
            level: snap.settings.booster.level,
            held,
            details,
            asking,
            told: snap.settings.booster.avatars_told,
            status: booster::status_line(snap).to_string(),
            now: booster::right_now(snap),
            comparison: report.comparisons.first().copied(),
            streamer: status.overlay.streamer,
            guide: report.guide.clone(),
            item_now: report.guide.item().map_or(String::new(), |item| booster::item_now(item, snap)),
            changed: booster::changed_now(snap),
            times: booster::times_words(snap),
            to_put_back: status.to_put_back.iter().map(booster::put_back_words).collect(),
            panel_3d: status.overlay.panel_3d,
            // Waiting is step 2's: below it there is nothing to compare.
            reconstruction: snap.reconstruction.is_some() && snap.settings.booster.level >= Level::Picture,
            test_tools: status.test_tools,
            test_load: status.test_load,
            holding: status.overlay.sharpness.is_some(),
        }
    }
}

const X: f32 = 24.0;
const WIDE: f32 = 976.0;
const CELL_Y: f32 = 12.0;
const CELL_HIGH: f32 = 92.0;
const CELL_GAP: f32 = 6.0;
const BUTTON_HIGH: f32 = 58.0;
const BOTTOM: f32 = 576.0 - 12.0 - BUTTON_HIGH;
const PANEL_Y: f32 = 214.0;

fn cell_wide() -> f32 {
    (WIDE - 3.0 * CELL_GAP) / 4.0
}

fn cell(i: usize) -> Rect {
    r(X + i as f32 * (cell_wide() + CELL_GAP), CELL_Y, cell_wide(), CELL_HIGH)
}

/// The stop at `x` on the page, wherever along the row it is: dragging across works past the ends too.
pub fn booster_level_at(x: f32) -> Level {
    let at = ((x - X) / (cell_wide() + CELL_GAP)).floor().clamp(0.0, 3.0) as usize;
    Level::ALL[at]
}

fn button_row(y: f32, buttons: Vec<(BoosterPick, &'static str, &'static str)>) -> Vec<Item> {
    let count = buttons.len().max(1) as f32;
    let wide = (WIDE - 10.0 * (count - 1.0)) / count;
    buttons
        .into_iter()
        .enumerate()
        .map(|(i, (pick, icon, label))| Item {
            action: Action::Set(Pick::Booster(pick)),
            rect: r(X + i as f32 * (wide + 10.0), y, wide, BUTTON_HIGH),
            look: Look::Choice { icon, label, on: false },
        })
        .collect()
}

pub(super) fn booster_items(facts: &Facts) -> Vec<Item> {
    let b = &facts.booster;
    let mut out = Vec::new();
    if b.details {
        // From the bottom up, as what is listed above takes what is left. Two to a row: these names are long in every language.
        let mut rows: Vec<Vec<(BoosterPick, &'static str, &'static str)>> = vec![vec![
            (BoosterPick::Details(false), icons::ARROW_LEFT, tr("Back to the slider")),
            (BoosterPick::PutBack, icons::ARROW_COUNTER_CLOCKWISE, tr("Put everything back")),
            (BoosterPick::OpenLogs, icons::FOLDER_OPEN, tr("Open the log folder")),
        ]];
        if b.test_tools {
            let tools = vec![
                (BoosterPick::HoldTest(!b.holding), icons::HAND_GRABBING, if b.holding { tr("Let go of the test change") } else { tr("Hold a test change") }),
                (BoosterPick::Crash, icons::WARNING, tr("Simulate crash")),
                (BoosterPick::TestLoad(!b.test_load), icons::GAUGE, if b.test_load { tr("Stop the test load") } else { tr("Start a test load") }),
                (BoosterPick::Report, icons::LIST_CHECKS, tr("Write a test report")),
            ];
            rows.extend(tools.chunks(2).rev().map(<[_]>::to_vec));
        }
        rows.extend(test_buttons(b).chunks(2).rev().map(<[_]>::to_vec));
        for (i, row) in rows.into_iter().enumerate() {
            out.extend(button_row(BOTTOM - i as f32 * (BUTTON_HIGH + 8.0), row));
        }
        return out;
    }
    for (i, level) in Level::ALL.into_iter().enumerate() {
        out.push(Item { action: Action::Set(Pick::Booster(BoosterPick::Level(level))), rect: cell(i), look: Look::BoosterCell(level) });
    }
    if b.asking {
        out.extend(button_row(
            BOTTOM,
            vec![(BoosterPick::Understood, icons::CHECK, tr("Show me how")), (BoosterPick::StayOnPicture, icons::X, tr("Stay on Picture"))],
        ));
    } else if b.level == Level::Avatars && b.guide.item().is_some() {
        let mut row = Vec::new();
        if b.guide.measuring.is_none() {
            row.push((BoosterPick::Done, icons::CHECK, tr("Done")));
        }
        row.push((BoosterPick::Previous, icons::ARROW_LEFT, tr("Previous")));
        row.push((BoosterPick::Next, icons::ARROW_RIGHT, tr("Next")));
        row.push((BoosterPick::Details(true), icons::LIST, tr("Details")));
        out.extend(button_row(BOTTOM, row));
    } else if !b.to_put_back.is_empty() {
        out.extend(button_row(
            BOTTOM,
            vec![(BoosterPick::PutBackDone, icons::CHECK, tr("I've put them back")), (BoosterPick::Details(true), icons::LIST, tr("Details"))],
        ));
    } else {
        out.extend(button_row(BOTTOM, vec![(BoosterPick::Details(true), icons::LIST, tr("Details"))]));
    }
    out
}

fn test_buttons(b: &BoosterFacts) -> Vec<(BoosterPick, &'static str, &'static str)> {
    let mut row = vec![
        (BoosterPick::Test(Kind::Sharpness), icons::EYE, tr("Test sharpness anyway")),
        (BoosterPick::Test(Kind::Programs), icons::GAUGE, tr("Test efficiency mode for programs")),
    ];
    if b.panel_3d {
        row.push((BoosterPick::Test(Kind::PanelQuality), icons::CUBE, tr("Test lighter 3D panels")));
    }
    if b.reconstruction {
        row.push((BoosterPick::Test(Kind::ReconstructionHold), icons::IMAGES, tr("Test 3D pictures waiting")));
    }
    row
}

/// A stop of the slider. The stops a level includes are partly filled, so that the row reads as how far it goes.
pub(super) fn draw_booster_cell(p: &Painter, rect: Rect, level: Level, facts: &Facts, hovered: bool, pressed: bool) {
    let b = &facts.booster;
    let shown = b.held.unwrap_or(b.level);
    let chosen = shown == level;
    let included = level < shown && level != Level::Off;
    let fill = if chosen {
        look::accent()
    } else if pressed {
        look::TILE_DOWN
    } else if hovered {
        look::TILE_HOVER
    } else if included {
        theme::mix(look::TILE, look::accent(), 0.32)
    } else {
        look::TILE
    };
    p.rect_filled(rect, look::radius(14.0), fill);
    let ink = if chosen { look::on_accent() } else { look::LABEL };
    let font = look::font(23.0, weight::BOLD);
    let name = fit(p, booster::level_name(level), &font, rect.width() - 24.0);
    p.text(rect.center() + vec2(0.0, 8.0), Align2::CENTER_CENTER, name, font, ink);
    // Off, then steps 1, 2 and 3: a slider in words too, for what colour cannot say in a monochrome look.
    let step = match level {
        Level::Off => "0",
        Level::Background => "1",
        Level::Picture => "2",
        Level::Avatars => "3",
    };
    p.text(rect.center() - vec2(0.0, 24.0), Align2::CENTER_CENTER, step, look::font(15.0, weight::SEMIBOLD), if chosen { ink } else { look::MUTED });
}

/// Text wrapped in a width, at most so many rows; where it ended.
fn lines(p: &Painter, at: Pos2, text: &str, size: f32, strength: f32, color: Color32, width: f32, rows: usize) -> f32 {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), look::font(size, strength), color, width);
    job.wrap.max_rows = rows;
    let galley = p.layout_job(job);
    let height = galley.size().y;
    p.galley(at, galley, color);
    at.y + height
}

pub(super) fn draw_booster(p: &Painter, facts: &Facts) {
    let b = &facts.booster;
    if b.details {
        draw_details(p, b);
        return;
    }
    let mut y = CELL_Y + CELL_HIGH + 10.0;
    y = lines(p, pos2(X + 4.0, y), booster::level_line(b.held.unwrap_or(b.level)), 19.0, weight::MEDIUM, look::MUTED, WIDE - 8.0, 2);
    y = lines(p, pos2(X + 4.0, y + 8.0), &b.status, 23.0, weight::BOLD, look::LABEL, WIDE - 8.0, 2);
    let panel = Rect::from_min_max(pos2(X, y.max(PANEL_Y - 8.0) + 8.0), pos2(X + WIDE, BOTTOM - 8.0));
    p.rect_filled(panel, look::radius(14.0), look::TILE);
    let inside = pos2(panel.left() + 18.0, panel.top() + 14.0);
    let width = panel.width() - 36.0;
    if b.asking {
        lines(p, inside, booster::avatars_question(), 22.0, weight::SEMIBOLD, look::LABEL, width, 4);
    } else if b.level == Level::Avatars {
        draw_guide(p, inside, width, b);
    } else if !b.to_put_back.is_empty() {
        let mut y = lines(p, inside, tr("Put these back in VRChat's menu if you want them as they were:"), 19.0, weight::SEMIBOLD, look::LABEL, width, 2);
        for line in b.to_put_back.iter().take(5) {
            y = lines(p, pos2(inside.x, y + 6.0), line, 17.0, weight::MEDIUM, look::MUTED, width, 1);
        }
    } else {
        draw_figures(p, inside, width, b);
    }
}

/// Before and after, only from a comparison that ran to its end; otherwise only what is so now.
fn draw_figures(p: &Painter, at: Pos2, width: f32, b: &BoosterFacts) {
    let mut y = at.y;
    if let Some(comparison) = &b.comparison {
        let said = booster::before_after(comparison);
        let title = format!("{} · {}", said.title, booster::kind_name(comparison.kind));
        y = lines(p, pos2(at.x, y), &title, 18.0, weight::MEDIUM, look::MUTED, width, 1);
        y = lines(p, pos2(at.x, y + 4.0), &said.figures, 46.0, weight::BOLD, look::LABEL, width, 1);
        y = lines(p, pos2(at.x, y + 4.0), said.said, 20.0, weight::MEDIUM, look::LABEL, width, 1);
        y = lines(p, pos2(at.x, y + 6.0), &said.filled, 17.0, weight::MEDIUM, look::MUTED, width, 1);
    } else if let Some(now) = &b.now {
        y = lines(p, pos2(at.x, y), now, 22.0, weight::SEMIBOLD, look::LABEL, width, 2);
    } else {
        y = lines(p, pos2(at.x, y), tr("Nothing measured yet."), 19.0, weight::MEDIUM, look::MUTED, width, 1);
    }
    if b.streamer {
        lines(p, pos2(at.x, y + 6.0), booster::streamer_line(), 16.0, weight::MEDIUM, look::MUTED, width, 1);
    }
}

/// Step 3's list: one item at a time. Its two figures only ever appear after Done, labelled as what they are.
fn draw_guide(p: &Painter, at: Pos2, width: f32, b: &BoosterFacts) {
    let mut y = at.y;
    if !b.guide.likely {
        y = lines(p, pos2(at.x, y), tr("Nothing here is likely to help right now."), 16.0, weight::MEDIUM, look::MUTED, width, 1) + 4.0;
    }
    if let Some(item) = b.guide.item() {
        y = lines(p, pos2(at.x, y), booster::item_name(item), 22.0, weight::SEMIBOLD, look::LABEL, width, 2);
        y = lines(p, pos2(at.x, y + 4.0), &b.item_now, 16.0, weight::MEDIUM, look::MUTED, width, 1);
    }
    if let Some((_, left)) = b.guide.measuring {
        y = lines(
            p,
            pos2(at.x, y + 6.0),
            &orbiters_i18n::tr!("Measuring for {seconds} more seconds.", seconds = left.ceil() as u32),
            16.0,
            weight::MEDIUM,
            look::LABEL,
            width,
            1,
        );
    }
    for m in b.guide.results.iter().rev().take(2) {
        y = lines(p, pos2(at.x, y + 6.0), &booster::measured_words(m), 15.0, weight::MEDIUM, look::MUTED, width, 2);
    }
}

fn draw_details(p: &Painter, b: &BoosterFacts) {
    let mut y = 14.0;
    y = lines(p, pos2(X, y), tr("What's changed right now"), 20.0, weight::BOLD, look::LABEL, WIDE, 1) + 6.0;
    if b.changed.is_empty() {
        y = lines(p, pos2(X, y), tr("Nothing is changed right now."), 17.0, weight::MEDIUM, look::MUTED, WIDE, 1) + 4.0;
    }
    for line in b.changed.iter().take(7) {
        y = lines(p, pos2(X, y), line, 17.0, weight::MEDIUM, look::MUTED, WIDE, 1) + 4.0;
    }
    if let Some(times) = &b.times {
        lines(p, pos2(X, y + 6.0), times, 17.0, weight::MEDIUM, look::LABEL, WIDE, 1);
    }
}

/// What this sheet's things do, where they are pointed at.
pub(super) fn booster_caption(action: Action) -> Option<(String, String)> {
    let (a, b): (&str, &str) = match action {
        Action::Booster => (tr("FPS booster"), tr("Spend what helps your frames. Shows what it really gains.")),
        Action::Set(Pick::Booster(BoosterPick::Level(level))) => (booster::level_name(level), booster::level_line(level)),
        Action::Set(Pick::Booster(_)) => return None,
        _ => return None,
    };
    Some((a.to_string(), b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::settings::Settings;

    fn facts_with(snap: &Snapshot, details: bool, asking: bool) -> Facts {
        Facts { sheet: Some(Sheet::Booster), booster: BoosterFacts::of(snap, details, asking, None), ..Facts::default() }
    }

    /// In a headset a cell is aimed at with a laser from arm's length: each must be larger than a Settings' choice, whose
    /// height is what the sheets already use, and all four on the page.
    #[test]
    fn the_cells_are_large_and_on_the_page() {
        let snap = Snapshot::new(Settings::default());
        let items = booster_items(&facts_with(&snap, false, false));
        let cells: Vec<&Item> = items.iter().filter(|i| matches!(i.look, Look::BoosterCell(_))).collect();
        assert_eq!(cells.len(), 4);
        for cell in cells {
            assert!(cell.rect.height() > 58.0 && cell.rect.width() > 200.0, "{:?}", cell.rect);
            assert!(cell.rect.right() <= 1024.0);
        }
        for item in &items {
            assert!(item.rect.bottom() <= 576.0 && item.rect.right() <= 1024.0, "{:?}", item.rect);
        }
    }

    #[test]
    fn dragging_across_lands_on_each_stop_and_past_the_ends() {
        assert_eq!(booster_level_at(0.0), Level::Off);
        assert_eq!(booster_level_at(cell(1).center().x), Level::Background);
        assert_eq!(booster_level_at(cell(2).center().x), Level::Picture);
        assert_eq!(booster_level_at(cell(3).center().x), Level::Avatars);
        assert_eq!(booster_level_at(1024.0), Level::Avatars);
    }

    #[test]
    fn nothing_overlaps_in_any_state() {
        let mut snap = Snapshot::new(Settings::default());
        snap.booster.test_tools = true;
        snap.settings.booster.level = Level::Avatars;
        snap.booster.overlay.report.guide.items = orbiters_booster::guided::list_for(orbiters_booster::classify::Limit::Processor);
        for facts in [facts_with(&snap, false, false), facts_with(&snap, true, false), facts_with(&snap, false, true)] {
            let items = booster_items(&facts);
            for (i, a) in items.iter().enumerate() {
                for b in &items[i + 1..] {
                    assert!(!a.rect.intersects(b.rect), "{:?} {:?}", a.rect, b.rect);
                }
                assert!(a.rect.top() >= 0.0 && a.rect.bottom() <= 576.0, "{:?}", a.rect);
            }
        }
    }

    /// Details with everything on it leaves room for what is listed above its buttons.
    #[test]
    fn details_leave_room_for_their_list() {
        let mut snap = Snapshot::new(Settings::default());
        snap.booster.test_tools = true;
        snap.booster.overlay.panel_3d = true;
        snap.settings.booster.level = Level::Picture;
        snap.reconstruction = None;
        let items = booster_items(&facts_with(&snap, true, false));
        let top = items.iter().map(|i| i.rect.top()).fold(f32::MAX, f32::min);
        assert!(top >= 200.0, "{top}");
    }

    /// One state: both surfaces read this, and the sheet shows the level the settings say.
    #[test]
    fn the_sheet_shows_what_the_settings_say() {
        let mut snap = Snapshot::new(Settings::default());
        snap.settings.booster.level = Level::Picture;
        assert_eq!(BoosterFacts::of(&snap, false, false, None).level, Level::Picture);
        assert_eq!(BoosterFacts::of(&snap, false, false, Some(Level::Off)).held, Some(Level::Off));
    }

    /// All Tools: the booster has its own tile before Settings, and the accessories all keep theirs.
    #[test]
    fn it_has_a_tile_in_all_tools_before_settings() {
        let tiles = accessories(&Facts::default());
        let at = |action: Action| tiles.iter().position(|t| t.0 == action);
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            assert_eq!(at(Action::Booster).unwrap() + 1, at(Action::Settings).unwrap());
        }
        let items = sheet_items(Sheet::All, &Facts { sheet: Some(Sheet::All), ..Facts::default() });
        for tile in items.iter().filter(|i| matches!(i.look, Look::Small { .. })) {
            assert!(tile.rect.width() > 100.0 && tile.rect.height() > 40.0, "accessories keep real room: {:?}", tile.rect);
        }
    }
}
