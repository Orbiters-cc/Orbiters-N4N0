//! The panel's pages past a capture's own: the labels the results are kept to, its world's labels to give, its
//! comments, a comment or a reason typed on the keyboard, and its pictures.

use super::super::super::orbit::panels::look as orbit_look;
use super::*;
use orbiters_i18n::{tr, tr_ctx, tr_n};

/// A label of the list on the labels page (three across).
fn label_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + (i % 3) as f32 * 328.0, 100.0 + (i / 3) as f32 * 64.0), vec2(304.0, 52.0))
}

/// The same on the filter page, under "Shared by its author".
fn filter_label_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + (i % 3) as f32 * 328.0, 166.0 + (i / 3) as f32 * 58.0), vec2(304.0, 50.0))
}

const OFFICIAL: Rect = Rect { min: Pos2 { x: 32.0, y: 100.0 }, max: Pos2 { x: 336.0, y: 150.0 } };
const HAS_3D: Rect = Rect { min: Pos2 { x: 360.0, y: 100.0 }, max: Pos2 { x: 664.0, y: 150.0 } };
const HEADER_LEFT: Rect = Rect { min: Pos2 { x: 744.0, y: 24.0 }, max: Pos2 { x: 864.0, y: 80.0 } };
const HEADER_RIGHT: Rect = Rect { min: Pos2 { x: 872.0, y: 24.0 }, max: Pos2 { x: 992.0, y: 80.0 } };
const WRITE: Rect = Rect { min: Pos2 { x: 744.0, y: 24.0 }, max: Pos2 { x: 992.0, y: 80.0 } };
const SEND: Rect = Rect { min: Pos2 { x: 824.0, y: 24.0 }, max: Pos2 { x: 992.0, y: 80.0 } };
/// Reads the world's feedback again, at the foot of its comments, where Orbiters did not answer.
const RETRY_FEEDBACK: Rect = Rect { min: Pos2 { x: 816.0, y: 514.0 }, max: Pos2 { x: 992.0, y: 562.0 } };
/// What a press on Write a comment did not do, and the way out, said under it (F6).
const WRITE_SAYS: Rect = Rect { min: Pos2 { x: 552.0, y: 90.0 }, max: Pos2 { x: 992.0, y: 176.0 } };
/// A picture shown whole.
const WHOLE: Rect = Rect { min: Pos2 { x: 32.0, y: 92.0 }, max: Pos2 { x: 992.0, y: 528.0 } };

fn comment_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0, 100.0 + i as f32 * 104.0), vec2(960.0, 92.0))
}

fn delete_rect(row: Rect) -> Rect {
    Rect::from_min_size(pos2(row.max.x - 132.0, row.center().y - 22.0), vec2(116.0, 44.0))
}

fn picture_rect(slot: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + (slot % 4) as f32 * 244.0, 100.0 + (slot / 4) as f32 * 224.0), vec2(228.0, 208.0))
}

/// Bottom paging (comments): Previous and Next either side of the page's number.
fn bottom_arrows(out: &mut Vec<(Target, Rect)>) {
    out.push((Target::Previous, Rect::from_center_size(pos2(440.0, 540.0), vec2(56.0, 56.0))));
    out.push((Target::Next, Rect::from_center_size(pos2(584.0, 540.0), vec2(56.0, 56.0))));
}

/// The words a reason has, and how many it needs.
fn words(draft: &str) -> (usize, usize) {
    let need = contract::labels().iter().find_map(|l| l.justify).unwrap_or(0);
    (contract::words(draft), need)
}

impl Panel {
    /// The world's comments on their page, and how many pages there are.
    fn comment_rows(&self, wf: &WorldFeedback) -> (usize, usize) {
        let pages = wf.comments.len().div_ceil(COMMENTS_PER_PAGE).max(1);
        (self.comments_page.min(pages - 1), pages)
    }

    pub(super) fn page_targets(&self, d: &Data, out: &mut Vec<(Target, Rect)>) {
        match self.page {
            Page::Filter => {
                if self.filtered() {
                    out.push((Target::ClearFilters, HEADER_LEFT));
                }
                out.push((Target::Done, HEADER_RIGHT));
                out.push((Target::FilterOfficial, OFFICIAL));
                out.push((Target::Has3d, HAS_3D));
                for i in 0..contract::labels().len() {
                    out.push((Target::FilterLabel(i as u8), filter_label_rect(i)));
                }
            }
            Page::Labels => {
                for i in 0..contract::labels().len() {
                    out.push((Target::Label(i as u8), label_rect(i)));
                }
            }
            Page::Comments => {
                let Some(e) = self.entry(d) else { return };
                out.push((Target::WriteComment, WRITE));
                let wf = d.world_feedback(e);
                // Orbiters did not answer its read: asked again from here.
                if wf.loading && wf.error.is_some() {
                    out.push((Target::RetryFeedback, RETRY_FEEDBACK));
                }
                let (page, pages) = self.comment_rows(&wf);
                for (i, c) in wf.comments.iter().skip(page * COMMENTS_PER_PAGE).take(COMMENTS_PER_PAGE).enumerate() {
                    if c.mine && c.id != 0 {
                        out.push((Target::DeleteComment(c.id), delete_rect(comment_rect(i))));
                    }
                }
                if pages > 1 {
                    bottom_arrows(out);
                }
            }
            Page::Compose(_) => {
                out.push((Target::Field, field_rect()));
                out.push((Target::Done, SEND));
                out.extend(keys(self.layout, true));
            }
            Page::Pictures => {
                let Some(f) = self.entry(d).and_then(|e| d.facts_of(e)) else { return };
                if let Some(i) = self.picture.filter(|i| *i < f.pictures.len()) {
                    out.push((Target::Picture(i), WHOLE));
                    return;
                }
                let pages = f.pictures.len().div_ceil(PICTURES_PER_PAGE).max(1);
                if pages > 1 {
                    out.push((Target::Previous, Rect::from_center_size(pos2(900.0, 52.0), vec2(56.0, 56.0))));
                    out.push((Target::Next, Rect::from_center_size(pos2(964.0, 52.0), vec2(56.0, 56.0))));
                }
                let first = self.pictures_page.min(pages - 1) * PICTURES_PER_PAGE;
                for i in first..(first + PICTURES_PER_PAGE).min(f.pictures.len()) {
                    out.push((Target::Picture(i), picture_rect(i - first)));
                }
            }
            Page::Browse | Page::Typing | Page::Detail | Page::Go => {}
        }
    }

    /// The page's title beside Back: `title`, then the world's name fainter.
    pub(super) fn title(&self, p: &Painter, title: &str, world: Option<&str>, until: f32, shown: f32) {
        let mut job = orbit_look::txt(title, 28.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)).job_truncated(until - 108.0);
        if let Some(w) = world {
            job.append(&format!("  {w}"), 0.0, orbit_look::txt("", 20.0).color(fade(theme::FG_3, shown)).format());
        }
        let galley = p.layout_job(job);
        p.galley(pos2(108.0, 52.0 - galley.size().y / 2.0), galley, theme::FG);
    }

    pub(super) fn draw_page(&self, p: &Painter, d: &Data, time: f64, targets: &[(Target, Rect)], shown: f32) {
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        let entry = self.entry(d);
        let name = entry.map(|e| e.name.as_str());
        match self.page {
            Page::Filter => {
                self.title(p, tr("Filter"), None, 730.0, shown);
                for (t, rect) in targets {
                    let (h, pr) = state(*t);
                    match *t {
                        Target::ClearFilters => pill_button(p, *rect, None, tr("Clear"), RAISED, theme::FG, h, pr, shown),
                        Target::Done => pill_button(p, *rect, None, tr("Done"), theme::ACCENT, theme::ON_ACCENT, h, pr, shown),
                        Target::FilterOfficial => {
                            let n = d.entries.iter().filter(|e| e.official).count() as u32;
                            label_chip(p, *rect, tr("Shared by its author"), Some(n), self.official, h, pr, shown);
                        }
                        Target::Has3d => {
                            let n = d.entries.iter().filter(|e| e.capture).count() as u32;
                            label_chip(p, *rect, tr("Has a 3D capture"), Some(n), self.has_3d, h, pr, shown);
                        }
                        Target::FilterLabel(i) => {
                            let Some(l) = contract::labels().get(i as usize) else { continue };
                            // How many listed worlds have it.
                            let n = d.entries.iter().filter(|e| d.label_count(e, &l.id) > 0).count() as u32;
                            label_chip(p, *rect, l.name(language_tag()), Some(n), self.labels.contains(&l.id), h, pr, shown);
                        }
                        _ => {}
                    }
                }
                text(p, pos2(32.0, 538.0), Align2::LEFT_CENTER, orbit_look::txt(tr("Every label picked: worlds members gave them all"), 15.0).color(fade(theme::FG_3, shown)));
                let n = self.results(d).len();
                let line = match n {
                    0 => tr("No world matches").to_string(),
                    n => tr_n("{count} world matches", "{count} worlds match", n as u64),
                };
                text(p, pos2(992.0, 538.0), Align2::RIGHT_CENTER, orbit_look::txt(line, 16.0).color(fade(theme::FG_3, shown)));
            }
            Page::Labels => {
                self.title(p, tr("Labels"), name, 990.0, shown);
                let Some(e) = entry else { return };
                let wf = d.world_feedback(e);
                for (t, rect) in targets {
                    if let Target::Label(i) = *t {
                        let Some(l) = contract::labels().get(i as usize) else { continue };
                        let (h, pr) = state(*t);
                        let count = wf.label(&l.id).map_or(0, |c| c.count);
                        label_chip(p, *rect, l.name(language_tag()), Some(count), wf.has(&l.id), h, pr, shown);
                    }
                }
                let hint = if !d.feedback.signed_in {
                    tr("Sign in to Orbiters - N4N0 to give feedback.").to_string() // the contract's Refusal::SignIn.text(), which a test below keeps equal
                } else if wf.loading {
                    tr("Reading this world's feedback from Orbiters…").to_string()
                } else {
                    tr("Press a label to give it, again to take it back. AI world asks why.").to_string()
                };
                let (line, tint) = match self.note(time).map(str::to_string).or_else(|| wf.error.clone()) {
                    Some(n) => (n, theme::WARN),
                    None => (hint, theme::FG_3),
                };
                text_cut(p, pos2(32.0, 492.0), 960.0, orbit_look::txt(line, 16.0).color(fade(tint, shown)));
                if let Some(reason) = wf.justification.as_ref().filter(|_| wf.has("ai-world")) {
                    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
                    text_cut(p, pos2(32.0, 524.0), 960.0, orbit_look::txt(tr!("Your reason: {reason}", reason = reason), 15.0).color(fade(theme::FG_4, shown)));
                }
            }
            Page::Comments => {
                let Some(e) = entry else { return };
                let wf = d.world_feedback(e);
                self.title(p, &tr!("Comments · {count}", count = wf.comment_count), Some(&e.name), 730.0, shown);
                let (page, pages) = self.comment_rows(&wf);
                for (t, rect) in targets {
                    let (h, pr) = state(*t);
                    match *t {
                        // Signed out it says what it does then: the way to sign in (F6).
                        Target::WriteComment if !d.feedback.signed_in => pill_button(p, *rect, Some(icons::SIGN_IN), tr("Sign in to comment"), theme::ACCENT, theme::ON_ACCENT, h, pr, shown),
                        Target::WriteComment => pill_button(p, *rect, Some(icons::PENCIL_SIMPLE), tr("Write a comment"), theme::ACCENT, theme::ON_ACCENT, h, pr, shown),
                        Target::RetryFeedback => pill_button(p, *rect, Some(icons::ARROW_CLOCKWISE), tr("Try again"), RAISED2, theme::FG, h, pr, shown),
                        Target::Previous => round_button(p, rect.center(), icons::CARET_LEFT, h, pr, shown),
                        Target::Next => round_button(p, rect.center(), icons::CARET_RIGHT, h, pr, shown),
                        _ => {}
                    }
                }
                if pages > 1 {
                    text(p, pos2(512.0, 540.0), Align2::CENTER_CENTER, orbit_look::txt(format!("{}/{}", page + 1, pages), 17.0).color(fade(theme::FG_3, shown)));
                }
                for (i, c) in wf.comments.iter().skip(page * COMMENTS_PER_PAGE).take(COMMENTS_PER_PAGE).enumerate() {
                    let row = comment_rect(i);
                    let appear = ease_out((time - self.page_at - i as f64 * CARD_STAGGER) / PAGE_FADE);
                    let row = row.translate(vec2(0.0, (1.0 - appear) * 12.0));
                    p.rect_filled(row, CornerRadius::same(20), fade(SURFACE, appear));
                    let by = if c.mine { tr("You") } else if c.by.is_empty() { tr("Someone") } else { c.by.as_str() };
                    let face = pos2(row.min.x + 38.0, row.min.y + 34.0);
                    p.circle_filled(face, 20.0, fade(theme::alpha(widgets::name_color(by), 0.3), appear));
                    let initial: String = by.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    text(p, face, Align2::CENTER_CENTER, orbit_look::txt(initial, 18.0).weight(weight::SEMIBOLD).color(fade(theme::FG, appear)));
                    let when = if c.id == 0 && c.mine { tr("sending…").to_string() } else { orbiters_n4n0_core::feedback::ago(d.now.saturating_sub(c.at)) };
                    let width = if c.mine { 740.0 } else { 860.0 };
                    text_cut(p, pos2(row.min.x + 72.0, row.min.y + 12.0), width, orbit_look::txt(format!("{by} · {when}"), 15.0).weight(weight::MEDIUM).color(fade(theme::FG_3, appear)));
                    let mut job = orbit_look::txt(c.text.split_whitespace().collect::<Vec<_>>().join(" "), 17.0).color(fade(theme::FG, appear)).line_height(23.0).job(width);
                    job.wrap.max_rows = 2;
                    p.galley(pos2(row.min.x + 72.0, row.min.y + 38.0), p.layout_job(job), theme::FG);
                    if c.mine && c.id != 0 {
                        let target = Target::DeleteComment(c.id);
                        let (h, pr) = state(target);
                        let asking = self.confirm.is_some_and(|(id, at)| id == c.id && time - at < CONFIRM_FOR);
                        let (bg, fg, label) = if asking { (theme::DANGER, theme::ON_ACCENT, tr("Delete?")) } else { (RAISED, theme::FG_2, tr("Delete")) };
                        icon_button(p, delete_rect(row), icons::TRASH, false, label, bg, fg, h, pr, appear);
                    }
                }
                let unread = wf.loading && wf.error.is_some();
                if wf.comments.is_empty() {
                    let (title, line) = if unread {
                        (tr("The comments could not be read"), tr("Orbiters did not answer. You can still write one."))
                    } else if wf.loading && wf.comment_count > 0 {
                        (tr("Reading the comments…"), tr("From Orbiters"))
                    } else {
                        (tr("No comment yet"), tr("Write the first one: what is there to do, who it is for"))
                    };
                    widgets::icon(p, pos2(512.0, 230.0), icons::CHAT_CIRCLE_TEXT, 44.0, fade(theme::FG_4, shown));
                    text(p, pos2(512.0, 286.0), Align2::CENTER_CENTER, orbit_look::txt(title, 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
                    text(p, pos2(512.0, 320.0), Align2::CENTER_CENTER, orbit_look::txt(line, 17.0).color(fade(theme::FG_3, shown)));
                }
                // What a press just did not do, under what was pressed, with the way out (F6): not in a corner of the page.
                if let Some(note) = self.note(time) {
                    let at = WRITE_SAYS;
                    p.rect_filled(at, CornerRadius::same(16), theme::INK_700);
                    p.rect_stroke(at, CornerRadius::same(16), Stroke::new(1.5, fade(theme::WARN, shown)), egui::StrokeKind::Inside);
                    // Its point, up to the button.
                    let tip = pos2(WRITE.center().x, at.min.y);
                    p.add(egui::Shape::convex_polygon(vec![tip - vec2(0.0, 9.0), tip + vec2(10.0, 1.0), tip - vec2(10.0, -1.0)], theme::INK_700, Stroke::NONE));
                    widgets::icon(p, pos2(at.min.x + 26.0, at.center().y), icons::INFO, 22.0, fade(theme::WARN, shown));
                    let mut job = orbit_look::txt(note.to_string(), 16.0).color(fade(theme::FG, shown)).line_height(21.0).job(at.width() - 66.0);
                    job.wrap.max_rows = 3;
                    let galley = p.layout_job(job);
                    p.galley(pos2(at.min.x + 48.0, at.center().y - galley.size().y / 2.0), galley, theme::FG);
                }
                if let Some(error) = wf.error.clone() {
                    let room = if unread { RETRY_FEEDBACK.min.x - 48.0 } else { 960.0 };
                    text_cut(p, pos2(32.0, 538.0), room, orbit_look::txt(error, 15.0).color(fade(theme::WARN, shown)));
                } else if wf.comment_count as usize > wf.comments.len() && !wf.comments.is_empty() {
                    let line = tr!("The newest {shown} of {total}", shown = wf.comments.len(), total = wf.comment_count);
                    text(p, pos2(32.0, 540.0), Align2::LEFT_CENTER, orbit_look::txt(line, 15.0).color(fade(theme::FG_4, shown)));
                }
            }
            Page::Compose(compose) => {
                let hint = if compose == Compose::Comment { tr("Write a comment") } else { tr("Say why") };
                field(p, field_rect(), &self.draft, hint, icons::PENCIL_SIMPLE, true, state(Target::Field), time, 1.0);
                let (valid, prompt, count) = match compose {
                    Compose::Comment => {
                        let n = self.draft.trim().chars().count();
                        let prompt = match name {
                            Some(world) => tr!("Your comment on {world}", world = world),
                            None => tr("Your comment on this world").to_string(),
                        };
                        (n > 0, prompt, tr!("{count} of {max} characters", count = n, max = contract::max_comment()))
                    }
                    Compose::Reason => {
                        let (have, need) = words(&self.draft);
                        let label = contract::labels().iter().find(|l| l.justify.is_some());
                        let prompt = label.and_then(|l| l.question(language_tag())).unwrap_or_else(|| tr("Why do you think so?")).to_string();
                        (have >= need, prompt, tr!("{have} of {need} words · public, beside the label", have = have, need = need))
                    }
                };
                let (h, pr) = state(Target::Done);
                // Signed out, Send is the way to sign in: what was typed stays (F6).
                let label = match compose {
                    Compose::Comment if !d.feedback.signed_in => tr_ctx("button", "Sign in"),
                    Compose::Comment => tr("Send"),
                    Compose::Reason => tr("Done"),
                };
                let (bg, fg) = if valid { (theme::ACCENT, theme::ON_ACCENT) } else { (RAISED, theme::FG_3) };
                pill_button(p, SEND, Some(icons::PAPER_PLANE_TILT), label, bg, fg, h, pr, 1.0);
                text_cut(p, pos2(36.0, 96.0), 952.0, orbit_look::txt(prompt, 19.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
                let tint = if valid { theme::ACCENT } else { theme::FG_3 };
                text(p, pos2(36.0, 136.0), Align2::LEFT_CENTER, orbit_look::txt(count, 15.0).color(fade(tint, shown)));
                if let Some(note) = self.note(time) {
                    // Beside the count, as wide as the line allows: a sign-in sentence is long (F6).
                    let mut job = orbit_look::txt(note.to_string(), 15.0).color(fade(theme::WARN, shown)).job(560.0);
                    job.halign = egui::Align::RIGHT;
                    job.wrap.max_rows = 2;
                    let galley = p.layout_job(job);
                    p.galley(pos2(988.0, 136.0 - galley.size().y / 2.0), galley, theme::WARN);
                }
                // The whole text, for a long one the field shows the end of.
                let mut job = orbit_look::txt(self.draft.clone(), 16.0).color(fade(theme::FG_2, shown)).line_height(21.0).job(952.0);
                job.wrap.max_rows = 3;
                p.galley(pos2(36.0, 158.0), p.layout_job(job), theme::FG_2);
                self.keyboard(p, targets, shown);
            }
            Page::Pictures => {
                let Some(e) = entry else { return };
                let facts = d.facts_of(e);
                let count = facts.map_or(e.pictures as usize, |f| f.picture_count.max(e.pictures as usize));
                self.title(p, &tr!("Pictures · {count}", count = count), Some(&e.name), 840.0, shown);
                let Some(f) = facts else {
                    text(p, pos2(512.0, 300.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Loading the pictures…"), 19.0).color(fade(theme::FG_3, shown)));
                    return;
                };
                if f.pictures.is_empty() {
                    widgets::icon(p, pos2(512.0, 250.0), icons::IMAGES, 44.0, fade(theme::FG_4, shown));
                    text(p, pos2(512.0, 306.0), Align2::CENTER_CENTER, orbit_look::txt(tr("No picture of this world yet"), 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
                    text(p, pos2(512.0, 340.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Members share theirs from Orbiters - N4N0's Worlds page"), 17.0).color(fade(theme::FG_3, shown)));
                    return;
                }
                let caption = |w: &WorldPicture| {
                    let mut parts = Vec::new();
                    if let Some(by) = &w.by {
                        parts.push(by.clone());
                    }
                    if let Some(date) = w.taken_at.as_deref().and_then(short_date) {
                        parts.push(date);
                    }
                    parts.join(" · ")
                };
                if let Some(i) = self.picture.filter(|i| *i < f.pictures.len()) {
                    let w = &f.pictures[i];
                    let aspect = match (w.width, w.height) {
                        (Some(w), Some(h)) if w > 0 && h > 0 => w as f32 / h as f32,
                        _ => 16.0 / 9.0,
                    };
                    // The whole picture, as large as it fits.
                    let fit = if WHOLE.width() / WHOLE.height() > aspect { vec2(WHOLE.height() * aspect, WHOLE.height()) } else { vec2(WHOLE.width(), WHOLE.width() / aspect) };
                    let rect = Rect::from_center_size(WHOLE.center(), fit);
                    match (d.picture)(w.id, true).or_else(|| (d.picture)(w.id, false)) {
                        Some(id) => cover(p, rect, id, aspect, CornerRadius::same(16), shown),
                        None => {
                            p.rect_filled(rect, CornerRadius::same(16), fade(SURFACE, shown));
                            text(p, rect.center(), Align2::CENTER_CENTER, orbit_look::txt(tr("Loading…"), 18.0).color(fade(theme::FG_3, shown)));
                        }
                    }
                    let place = tr!("{number} of {total}", number = i + 1, total = f.pictures.len());
                    let line = match caption(w) {
                        caption if caption.is_empty() => place,
                        caption => format!("{caption}  ·  {place}"),
                    };
                    text(p, pos2(512.0, 552.0), Align2::CENTER_CENTER, orbit_look::txt(line, 16.0).color(fade(theme::FG_3, shown)));
                    return;
                }
                let pages = f.pictures.len().div_ceil(PICTURES_PER_PAGE).max(1);
                let current = self.pictures_page.min(pages - 1);
                for (t, rect) in targets {
                    let (h, pr) = state(*t);
                    match *t {
                        Target::Previous => round_button(p, rect.center(), icons::CARET_LEFT, h, pr, 1.0),
                        Target::Next => round_button(p, rect.center(), icons::CARET_RIGHT, h, pr, 1.0),
                        Target::Picture(i) => {
                            let slot = i - current * PICTURES_PER_PAGE;
                            let appear = ease_out((time - self.page_at - slot as f64 * CARD_STAGGER) / PAGE_FADE);
                            let rect = rect.translate(vec2(0.0, (1.0 - appear) * 14.0));
                            let rect = if pr { rect.shrink(3.0) } else if h { rect.expand(2.0) } else { rect };
                            let w = &f.pictures[i];
                            let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), 160.0));
                            match (d.picture)(w.id, false) {
                                Some(id) => {
                                    let aspect = match (w.width, w.height) {
                                        (Some(w), Some(h)) if w > 0 && h > 0 => w as f32 / h as f32,
                                        _ => 16.0 / 9.0,
                                    };
                                    cover(p, thumb, id, aspect, CornerRadius::same(14), appear);
                                }
                                None => {
                                    p.rect_filled(thumb, CornerRadius::same(14), fade(if h { RAISED } else { SURFACE }, appear));
                                }
                            }
                            text_cut(p, pos2(rect.min.x + 4.0, rect.min.y + 170.0), rect.width() - 8.0, orbit_look::txt(caption(w), 14.0).color(fade(theme::FG_3, appear)));
                        }
                        _ => {}
                    }
                }
                if pages > 1 {
                    text(p, pos2(860.0, 52.0), Align2::RIGHT_CENTER, orbit_look::txt(format!("{}/{}", current + 1, pages), 17.0).color(theme::FG_3));
                }
                if f.picture_count > f.pictures.len() {
                    let line = tr!("The newest {shown} of {total}: all of them on orbiters.cc", shown = f.pictures.len(), total = f.picture_count);
                    text(p, pos2(512.0, 552.0), Align2::CENTER_CENTER, orbit_look::txt(line, 15.0).color(fade(theme::FG_4, shown)));
                }
            }
            Page::Browse | Page::Typing | Page::Detail | Page::Go => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sign-in line is written out here (a text the catalogs can be checked against); the contract's own sentence is the one it must equal.
    #[test]
    fn the_sign_in_line_is_the_contracts_refusal() {
        assert_eq!(tr("Sign in to Orbiters - N4N0 to give feedback."), contract::Refusal::SignIn.text());
    }
}
