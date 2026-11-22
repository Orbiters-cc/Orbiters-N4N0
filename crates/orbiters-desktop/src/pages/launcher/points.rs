//! The member's Explorer points on Home (REQUEST19 1: "I would like the explorer points to be on the home page tab as
//! well"): a chip on the banner of the account's own profile card, beside its Change banner. It reads the same standing as
//! Settings' account card ([`crate::controller::explorer::Status::points`], read from Orbiters by the tray service) and says
//! it the same way: the title in the accent, the points, what the next title takes, and the thin line of the way to it.
//! Pressed, it goes to Settings, where the points live with the switch for the notice in worlds without a 3D picture.
//!
//! Signed out of Orbiters there are no points to show (as in Settings): no chip. Signed in before Orbiters said them
//! (or while it cannot be reached): the chip names Explorer points and waits, without a number.

use crate::controller::explorer as ex;
use crate::state::{AccountState, Snapshot};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_core::explorer::Explorer;
use orbiters_i18n::{mark, tr};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets;

/// What the chip says how points are earned, under where the member stands.
pub const HOW_EARNED: &str = mark("Earned by sharing the first public 3D pictures of a world. Press to see them in Settings.");

/// The chip's height and its insets.
const H: f32 = 34.0;
const PAD: f32 = 12.0;
const ICON: f32 = 17.0;

/// What the chip shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Chip {
    /// Signed in, Orbiters has not said them yet (or cannot be reached).
    Waiting,
    /// Where the member stands: the title (in the language now), the points in words, what the next title takes, and how
    /// far along the way to it (0 to 1).
    Standing { title: Option<String>, points: String, next: Option<String>, progress: f32 },
}

/// The chip for this snapshot: none while signed out of Orbiters (Settings shows the points only signed in, too).
pub fn chip(snap: &Snapshot) -> Option<Chip> {
    let signed_in = matches!(snap.account.state, AccountState::SignedIn { .. } | AccountState::Offline { username: Some(_) });
    if !signed_in {
        return None;
    }
    Some(match snap.explorer.points.as_ref() {
        None => Chip::Waiting,
        Some(e) => standing(e),
    })
}

/// Where `e` stands, in the words Settings uses ([`ex::standing_words`]'s parts).
fn standing(e: &Explorer) -> Chip {
    let next = e.next_title().map(|next| {
        let title = orbiters_core::explorer::Title { key: next.key.clone(), points: next.points, name: next.name.clone() };
        orbiters_i18n::tr!("{remaining} to {title}", remaining = orbiters_ui::format::points(next.remaining), title = ex::title_name(&title))
    });
    // Towards the next title as the list names it when the answer left `next` out (an older answer), as the words do.
    let progress = Explorer { next: e.next_title(), ..e.clone() }.progress();
    Chip::Standing { title: e.title.as_ref().map(ex::title_name), points: ex::points_words(e.points), next, progress }
}

impl Chip {
    /// What it says in one line: "Explorer · 12.5 points · 37.5 to Navigator".
    pub fn line(&self) -> String {
        match self {
            Chip::Waiting => tr(ex::EXPLORER_POINTS).to_string(),
            Chip::Standing { title, points, next, .. } => [title.as_deref(), Some(points.as_str()), next.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · "),
        }
    }

    /// What its hover says.
    fn hint(&self) -> String {
        match self {
            Chip::Waiting => format!("{}\n{}", tr(ex::EXPLORER_POINTS), tr("Shown once Orbiters has said them")),
            Chip::Standing { .. } => format!("{}: {}\n{}", tr(ex::EXPLORER_POINTS), self.line(), tr(HOW_EARNED)),
        }
    }
}

/// The chip drawn from `at` (its top left) in at most `room` across, on the banner; whether it was pressed.
pub fn draw(ui: &mut Ui, at: egui::Pos2, room: f32, chip: &Chip) -> bool {
    let ctx = ui.ctx().clone();
    let (title, rest, rest_color) = match chip {
        Chip::Waiting => (None, tr(ex::EXPLORER_POINTS).to_string(), theme::FG_3),
        Chip::Standing { title, points, next, .. } => {
            let rest = [Some(points.as_str()), next.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            (title.clone(), rest, theme::FG)
        }
    };
    let title_txt = title.as_ref().map(|t| Txt::new(t, 13.0).weight(weight::SEMIBOLD).color(theme::ACCENT));
    let rest_txt = Txt::new(rest, 13.0).weight(weight::MEDIUM).color(rest_color);
    let dot = Txt::new(" · ", 13.0).color(theme::FG_3);
    let title_w = title_txt.as_ref().map_or(0.0, |t| widgets::text_size(ui, t).x + widgets::text_size(ui, &dot).x);
    let rest_w = widgets::text_size(ui, &rest_txt).x;
    let lead = PAD + ICON + 8.0;
    let wide = (lead + title_w + rest_w + PAD).min(room.max(lead + 40.0 + PAD));
    let rect = Rect::from_min_size(at, vec2(wide, H));
    let id = Id::unique("home-explorer-points");
    let resp = ui.interact(rect, id, Sense::click());
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(H / 2.0), theme::black(0.5 + 0.2 * hover));
    // In the middle of the chip's height: beside the text and its line together, not the text alone.
    widgets::icon(&p, pos2(rect.left() + PAD + ICON / 2.0, rect.center().y), icons::COMPASS, ICON, theme::ACCENT);
    let mut x = rect.left() + lead;
    let y = rect.top() + 15.0;
    let end = rect.right() - PAD;
    if let Some(t) = title_txt.as_ref() {
        let r = widgets::text_truncated(&p, pos2(x, y), Align2::LEFT_CENTER, t, end - x);
        x = r.right();
        if x < end {
            x = widgets::text(&p, pos2(x, y), Align2::LEFT_CENTER, &dot).right();
        }
    }
    if x < end - 8.0 {
        widgets::text_truncated(&p, pos2(x, y), Align2::LEFT_CENTER, &rest_txt, end - x);
    }
    // The way to the next title, as Settings draws it: a thin line, the accent as far as the member got.
    let track = Rect::from_min_max(pos2(rect.left() + lead, rect.bottom() - 8.0), pos2(end, rect.bottom() - 5.0));
    p.rect_filled(track, theme::radius(1.5), theme::white(0.14));
    if let Chip::Standing { progress, .. } = chip {
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * progress.clamp(0.0, 1.0), track.height())), theme::radius(1.5), theme::ACCENT);
    }
    widgets::note_control(&ctx, &chip.line(), rect, true);
    let pressed = widgets::activated(&resp);
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(chip.hint());
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::explorer::{Next, Title};

    fn snap(state: AccountState, points: Option<Explorer>) -> Snapshot {
        let mut s = Snapshot::new(orbiters_core::settings::Settings::default());
        s.account.state = state;
        s.explorer.points = points;
        s
    }

    fn title(key: &str, points: f64) -> Title {
        Title { key: key.into(), points, name: key.into() }
    }

    /// REQUEST19 1: the points on Home say what Settings says, in each state: signed out nothing, signed in and not said
    /// yet a waiting chip without a number, zero points the way to the first title, a title with the way to the next, and
    /// the top title alone; in the language chosen.
    #[test]
    fn home_points_follow_the_account_and_what_orbiters_said() {
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            let signed_in = || AccountState::SignedIn { username: "Sample".into() };
            assert_eq!(chip(&snap(AccountState::SignedOut, Some(Explorer::default()))), None, "signed out: no points shown");
            assert_eq!(chip(&snap(AccountState::Expired { username: Some("Sample".into()) }, None)), None);
            assert_eq!(chip(&snap(AccountState::Unknown, None)), None);
            assert_eq!(chip(&snap(signed_in(), None)), Some(Chip::Waiting), "signed in, not said yet");
            assert_eq!(chip(&snap(AccountState::Offline { username: Some("Sample".into()) }, None)), Some(Chip::Waiting), "offline keeps the account");
            assert_eq!(chip(&snap(signed_in(), None)).unwrap().line(), "Explorer points");

            let titles = vec![title("explorer", 10.0), title("navigator", 50.0)];
            let zero = Explorer { points: 0.0, next: Some(Next { key: "explorer".into(), points: 10.0, name: "Explorer".into(), remaining: 10.0 }), titles: titles.clone(), ..Default::default() };
            let zero = chip(&snap(signed_in(), Some(zero))).unwrap();
            assert_eq!(zero.line(), "0 points · 10 to Explorer");
            assert!(matches!(zero, Chip::Standing { title: None, progress, .. } if progress == 0.0));

            let mid = Explorer { points: 12.5, title: Some(title("explorer", 10.0)), titles: titles.clone(), ..Default::default() };
            let mid = chip(&snap(signed_in(), Some(mid))).unwrap();
            assert_eq!(mid.line(), "Explorer · 12.5 points · 37.5 to Navigator");
            let Chip::Standing { progress, .. } = mid else { panic!() };
            assert!((progress - 2.5 / 40.0).abs() < 1e-4, "{progress}");

            let top = Explorer { points: 60.0, title: Some(title("navigator", 50.0)), titles, ..Default::default() };
            assert_eq!(chip(&snap(signed_in(), Some(top))).unwrap().line(), "Navigator · 60 points");
            let one = Explorer { points: 1.0, ..Default::default() };
            assert_eq!(chip(&snap(signed_in(), Some(one))).unwrap().line(), "1 point");
        });
        orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || {
            let e = Explorer { points: 12.5, title: Some(title("explorer", 10.0)), titles: vec![title("explorer", 10.0), title("navigator", 50.0)], ..Default::default() };
            assert_eq!(chip(&snap(AccountState::SignedIn { username: "Sample".into() }, Some(e))).unwrap().line(), "Explorateur · 12,5 points · 37,5 avant Navigateur");
        });
    }
}
