//! Settings page (REQUEST21 2): every setting of the app, in sub-tabs (a side list, a row of tabs in a narrow window) with
//! a search over all of them, as the Orbiters website's admin page has them. Each setting is in the registry
//! ([`super::settings_registry`]) once: the search lists it with its sub-tab and card, a result or a link from elsewhere
//! (`State::open`) opens its sub-tab, scrolls it into view and lights it briefly. Changes apply as they are made (shown
//! at once, saved in the background); text fields apply with their Save button (see [`super::form`]).

use super::form::{self, Texts};
use super::mic::{mic_test, MicTest};
use super::settings_registry::{self as registry, Entry, Found, Group, Open};
use super::{section, Cx, Job};
use crate::backend::Ctl;
use crate::ipc::UpdateAction;
use crate::state::{AccountState, Snapshot, UpdateState};
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_core::settings::{CoverageOverlay, RigViews, Settings, ViewGpu, WhilePlaying, WindowGpu};
use orbiters_i18n::{fill, mark, tr, tr_ctx, tr_n};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};
use orbiters_ui::{format, icons, toast};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Sessions,
    Raw,
}

#[derive(Default)]
pub struct State {
    vrchat: VrchatForm,
    /// The Spout sender, the record parameter and N4N0's AFK message as typed, until saved.
    texts: Texts,
    log_open: bool,
    picker: Option<(Folder, Job<Option<PathBuf>>)>,
    mic: MicTest,
    /// The microphone N4N0 listens to, tested.
    ears_mic: MicTest,
    mic_grid: super::mic_grid::State,
    /// "Sign out" was pressed once: it asks before signing every tool out.
    confirm_sign_out: bool,
    /// Whether Windows starts the app: read when the page opens (on the visit it was read), set at once by its switch
    /// while the registry is written.
    windows_start: Option<bool>,
    windows_read: u64,
    windows_job: Option<Job<Result<(), String>>>,
    /// The start with SteamVR as switched, until the settings say so, and SteamVR being told meanwhile.
    steamvr_shown: Option<bool>,
    steamvr_job: Option<(bool, Job<Result<bool, String>>)>,
    /// "When SteamVR starts": Windows' sound devices in VR.
    vr_audio: super::vr_audio::State,
    /// VRChat on the network, where it does not run on this computer.
    network: super::network::State,
    /// Experimental: the VRChat bridge.
    experimental: super::experimental::State,
    /// The FPS booster's card.
    booster: super::booster_card::State,
    /// Beta feedback: the menu mismatch buttons and their reports.
    beta: super::beta_feedback::State,
    /// The sub-tab shown: kept while the app runs, whatever page is visited in between.
    group: Group,
    /// When it was chosen (its cards enter from then), and how many times one was.
    group_since: f64,
    group_visits: u64,
    /// The sub-tab drawn last frame: another one starts at its top.
    drawn: Option<Group>,
    /// What is typed in the search.
    query: String,
    /// The search as it was last frame (a new one starts at the top of its results).
    searched: Option<String>,
    /// The result the arrow keys are on, and how many results show.
    active: usize,
    limit: usize,
    /// The search field, to know when it has the keys.
    search_id: Option<Id>,
    /// A setting to bring into view and light, once its sub-tab draws it.
    jump: Option<Jump>,
    /// The setting the search opened, said above its sub-tab until dismissed.
    found: Option<&'static str>,
    /// How many of the notices the log shows.
    log_shown: usize,
    /// Tests: the rows drawn last frame (their card's English title, their label, where), and the cards.
    /// The arrows moved the lit result this frame: it is brought into view.
    keyed: bool,
    /// How many of the worker log's last lines show (older ones a press of Show more away).
    worker_shown: usize,
    #[cfg(test)]
    pub(super) seen: Vec<(&'static str, String, Rect)>,
    #[cfg(test)]
    pub(super) seen_cards: Vec<(&'static str, Rect)>,
    /// Tests: the controls drawn last frame (their card's English title, their key, where).
    #[cfg(test)]
    pub(super) seen_controls: Vec<(&'static str, String, Rect)>,
    /// Tests: the kind of each of those controls (toggle, slider, button…), in the same order.
    #[cfg(test)]
    pub(super) seen_kinds: Vec<&'static str>,
    /// Tests: each row drawn last frame: its label, the room it gave its control, the width left to its text.
    #[cfg(test)]
    pub(super) seen_fields: Vec<(String, Rect, f32)>,
    /// Tests: where each result was drawn last frame.
    #[cfg(test)]
    pub(super) result_rects: Vec<Rect>,
}

/// A setting being brought into view: when it was lit, and the frames it was waited for.
#[derive(Clone, Copy, Debug)]
struct Jump {
    id: &'static str,
    lit: Option<f64>,
    waited: u32,
    /// Brought into view without being lit (screenshots of older scenes).
    quiet: bool,
}

impl State {
    /// Ends the mic test (leaving the page).
    pub fn stop_mic_test(&mut self) {
        self.mic = MicTest::default();
        self.ears_mic = MicTest::default();
        self.mic_grid.stop();
        self.vr_audio.stop();
        self.vrchat.password.clear();
        self.vrchat.code.clear();
    }

    /// Opens Settings on `target`: a setting's id (`account.vrchat`, its sub-tab shows, it comes into view and is lit) or
    /// a sub-tab's (`vr`). Whatever was searched gives way. False when there is no such setting or sub-tab.
    pub fn open(&mut self, target: &str, now: f64) -> bool {
        let (group, jump) = match (registry::entry(target), Group::of_id(target)) {
            (Some(entry), _) => (entry.group, Some(entry.id)),
            (None, Some(group)) => (group, None),
            (None, None) => return false,
        };
        self.choose_group(group, now);
        self.jump = jump.map(|id| Jump { id, lit: None, waited: 0, quiet: false });
        true
    }

    /// The sub-tab shown (tests, screenshots).
    pub fn group(&self) -> Group {
        self.group
    }

    fn choose_group(&mut self, group: Group, now: f64) {
        if group != self.group || !self.query.is_empty() {
            self.group_since = now;
            self.group_visits += 1;
        }
        self.group = group;
        self.query.clear();
        self.found = None;
        self.jump = None;
    }

    /// Screenshots: something typed in the search.
    pub(crate) fn stage_search(&mut self, query: &str) {
        self.query = query.to_owned();
    }

    /// Screenshots: the setting `id` as the search opened it (said above its sub-tab).
    pub(crate) fn stage_found(&mut self, id: &str) {
        self.found = registry::entry(id).map(|e| e.id);
    }

    /// Screenshots of the scenes made before the sub-tabs: the setting they show comes into view, unlit.
    pub(crate) fn quietly(&mut self) {
        if let Some(jump) = self.jump.as_mut() {
            jump.quiet = true;
        }
    }
}

/// What a sub-tab's cards asked of the page while they were drawn.
#[derive(Default)]
struct Asked {
    apply: bool,
    free_space: bool,
    want_log: bool,
    quit: bool,
}

/// The sub-tabs are a side list while the cards keep this much beside it: below, a row of tabs over the content.
const CONTENT_MIN: f32 = 600.0;
/// The side list is as wide as its longest name needs (each language its own), within these.
const SIDE_MIN: f32 = 196.0;
const SIDE_MAX: f32 = 272.0;
const SIDE_GAP: f32 = 28.0;
const SIDE_ITEM: f32 = 40.0;
/// How many results show before "Show more results" (the website's twelve).
const RESULTS: usize = 12;
/// How many notices the log shows before "Show more".
const LOG_FIRST: usize = 60;

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    // The controls edit a copy of the settings as published; what they change is saved over the settings as they
    // are by then (`form::save`).
    let base = cx.snap.settings.clone();
    let mut draft = base.clone();
    let mut asked = Asked::default();

    // A folder chosen in the picker.
    if let Some((which, job)) = state.picker.as_mut() {
        if let Some(choice) = job.take() {
            if let Some(dir) = choice {
                match which {
                    Folder::Sessions => draft.sessions_dir = dir,
                    Folder::Raw => draft.raw_captures_dir = dir,
                }
                asked.apply = true;
            }
            state.picker = None;
        }
    }

    // Starting with Windows: read on each visit; a write that failed shows what is.
    if state.windows_read != cx.visit {
        state.windows_read = cx.visit;
        // Tests never read this computer's registry.
        state.windows_start = Some(!cfg!(test) && crate::startup::starts_with_windows());
    }
    if let Some(result) = state.windows_job.as_mut().and_then(Job::take) {
        state.windows_job = None;
        if let Err(e) = result {
            cx.toasts.push(&ctx, toast::Kind::Error, tr!("The start with {system} did not change: {error}", system = crate::words::SYSTEM, error = e));
            state.windows_start = Some(crate::startup::starts_with_windows());
        }
    }
    // Starting with SteamVR: told to SteamVR when it runs, kept for its next start otherwise.
    if let Some((on, result)) = state.steamvr_job.as_mut().and_then(|(on, job)| job.take().map(|r| (*on, r))) {
        state.steamvr_job = None;
        match result {
            Ok(told) => {
                draft.startup.steamvr = on;
                draft.startup.steamvr_pending = !told;
                asked.apply = true;
            }
            Err(e) => {
                state.steamvr_shown = None;
                cx.toasts.push(&ctx, toast::Kind::Error, tr!("SteamVR did not take the change: {error}", error = e));
            }
        }
    }
    if state.steamvr_job.is_none() && state.steamvr_shown == Some(base.startup.steamvr) {
        state.steamvr_shown = None;
    }

    let open = open_here(cx);
    let groups = registry::groups(&open);
    if !groups.contains(&state.group) {
        state.group = groups.first().copied().unwrap_or_default();
    }

    let page = ui.max_rect();
    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    // Room for the longest name, its icon and a count of results.
    let longest = groups.iter().map(|g| widgets::text_size(ui, &Txt::new(g.title(), 13.5).weight(weight::MEDIUM)).x).fold(0.0, f32::max);
    let side_w = (longest + 42.0 + 14.0 + 36.0).clamp(SIDE_MIN, SIDE_MAX);
    let side = head.width() - side_w - SIDE_GAP >= CONTENT_MIN;
    let tabs_r = Rect::from_min_size(pos2(head.left(), head.bottom() + 14.0), vec2(head.width(), 40.0));
    // Under a row of tabs, room enough for what scrolls up under them to be fully blurred by the time it reaches them.
    let content_top = if side { head.bottom() + 20.0 } else { tabs_r.bottom() + 36.0 };
    let content_left = if side { head.left() + side_w + SIDE_GAP } else { head.left() };
    let search_w = (head.width() * 0.48).clamp(220.0, 400.0);
    let search_r = Rect::from_min_size(pos2(head.right() - search_w, head.bottom() - 40.0), vec2(search_w, 40.0));

    // The search: Ctrl (⌘) K takes the keys there; while it has them, the arrows move along the results, Enter opens
    // one and Escape clears it (the website's keys).
    let words = registry::words(&state.query);
    let found = registry::search(&state.query, &open);
    let searching = !words.is_empty();
    state.limit = state.limit.max(RESULTS);
    let search_focused = state.search_id.is_some_and(|id| ctx.memory(|m| m.has_focus(id)));
    if search_focused && searching {
        let (down, up) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
        let shown = found.len().min(state.limit);
        if shown > 0 {
            if down {
                state.active = (state.active + 1) % shown;
            }
            if up {
                state.active = (state.active + shown - 1) % shown;
            }
            state.keyed |= down || up;
        }
    }
    let focus_search = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K));
    if super::new_search(&mut state.searched, &state.query) {
        state.active = 0;
        state.limit = RESULTS;
    }
    state.active = state.active.min(found.len().min(state.limit).saturating_sub(1));

    let from = orbiters_ui::blur::next_shape(ui);
    let scroll_region = Rect::from_min_max(pos2(content_left - 16.0, page.top()), page.max);
    let mut su = ui.new_child(UiBuilder::new().max_rect(scroll_region).id_salt("settings-content"));
    su.set_clip_rect(scroll_region.intersect(ui.clip_rect()));
    let mut scroll = egui::ScrollArea::vertical().id_salt(("settings", if searching { None } else { Some(state.group) })).auto_shrink([false, false]);
    if state.drawn != Some(state.group) && !searching {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut chosen: Option<&'static Entry> = None;
    let mut cleared = false;
    let bar = Rect::from_min_max(pos2(page.right() - 14.0, content_top), page.max);
    scroll.scroll_bar_rect(bar).show(&mut su, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: super::PAD_X as i8, top: 0, bottom: 40 }).show(ui, |ui| {
            ui.add_space(content_top - page.top());
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            if searching {
                results(ui, cx, state, &found, &words, &mut chosen, &mut cleared);
            } else {
                // A new sub-tab's cards enter again (counted from when it was chosen).
                let (entered, visit) = (cx.entered, cx.visit);
                cx.entered = cx.entered.max(state.group_since);
                cx.visit = visit.wrapping_mul(1009).wrapping_add(state.group_visits);
                group_page(ui, cx, state, &base, &mut draft, &mut asked, now);
                cx.entered = entered;
                cx.visit = visit;
            }
        });
    });
    state.drawn = (!searching).then_some(state.group);
    // Down to where the content starts.
    super::under_top(&su, cx, "settings", from, content_top - 6.0);

    // Above what scrolls: the heading, the search, the sub-tabs.
    super::heading_in(ui, head, tr("Preferences"), tr("Settings"), None, head.width() - search_w - 24.0);
    let mut su = ui.new_child(UiBuilder::new().max_rect(search_r).id_salt("settings-search"));
    let resp = widgets::search_field_clearable(&mut su, "settings-search-field", &mut state.query, tr("Search every setting…"), search_w);
    state.search_id = Some(resp.id);
    if focus_search {
        resp.request_focus();
    }
    if state.query.is_empty() && !resp.has_focus() {
        let keys = if cfg!(target_os = "macos") { "⌘ K" } else { "Ctrl K" }; // i18n: not shown (key names, as on the keyboard)
        let t = Txt::new(keys, 11.0).color(theme::FG_4);
        let s = widgets::text_size(ui, &t);
        let chip = Rect::from_min_size(pos2(search_r.right() - 14.0 - s.x - 12.0, search_r.center().y - 10.0), vec2(s.x + 12.0, 20.0));
        ui.painter().rect_filled(chip, theme::radius(6.0), theme::white(0.05));
        widgets::text(ui.painter(), chip.center(), Align2::CENTER_CENTER, &t);
    }
    if resp.lost_focus() {
        let (enter, escape) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter && searching {
            chosen = chosen.or(found.get(state.active).map(|f| f.entry));
        }
        if escape {
            cleared = true;
        }
    }
    let counts: Vec<usize> = groups.iter().map(|g| if searching { found.iter().filter(|f| f.entry.group == *g).count() } else { 0 }).collect();
    let picked = if side {
        let r = Rect::from_min_size(pos2(head.left(), content_top), vec2(side_w, page.bottom() - content_top));
        side_list(ui, r, &groups, state.group, searching, &counts)
    } else {
        tab_row(ui, tabs_r, &groups, state.group, searching, &counts)
    };
    if let Some(group) = picked {
        state.choose_group(group, now);
    }
    if let Some(entry) = chosen {
        // Hidden until another setting allows it: that setting is the one shown and lit.
        let target = target_of(entry, cx.snap);
        state.choose_group(entry.group, now);
        state.jump = Some(Jump { id: target.id, lit: None, waited: 0, quiet: false });
        state.found = Some(entry.id);
    } else if cleared {
        state.query.clear();
    }

    if asked.free_space {
        cx.go = Some(super::Go::FreeSpace);
    }
    if asked.want_log {
        state.log_open = !state.log_open;
        if state.log_open {
            cx.ctl.request_worker_log();
        }
    }
    if asked.quit {
        cx.ctl.quit();
    }
    if asked.apply {
        form::save(cx.ctl, &base, &draft);
    }
}

/// Which of the registry's gates are open here.
fn open_here(cx: &Cx) -> Open {
    open_of(cx.snap, cx.gpu.map(|g| &g.adapter).filter(|a| a.choice).map(|a| a.integrated))
}

/// [`open_here`] for a snapshot, and whether the window's GPU can be chosen (and is the integrated one).
fn open_of(snap: &Snapshot, gpu_integrated: Option<bool>) -> Open {
    let status = &snap.vrchat_login;
    let orbiters = matches!(snap.account.state, AccountState::SignedIn { .. } | AccountState::Checking { .. } | AccountState::Offline { .. });
    // As `vrchat_account` decides to draw its card (a saved session being restored shows too).
    let connecting = status.presence() == crate::controller::vrchat_login::Presence::Connecting && status.methods.is_empty();
    Open {
        bridge: snap.bridge.allowed,
        vrchat_login: (status.allowed || status.waiting || connecting) && orbiters,
        gpu_choice: gpu_integrated.is_some(),
        gpu_split: gpu_integrated == Some(false),
    }
}

/// The sub-tabs down the side, the one shown lit (the app's own navigation, smaller): the one pressed, if any. While a
/// search is typed none is lit, and each says how many of the results are in it.
fn side_list(ui: &mut Ui, rect: Rect, groups: &[Group], shown: Group, searching: bool, counts: &[usize]) -> Option<Group> {
    let ctx = ui.ctx().clone();
    let id = Id::unique("settings-side-list");
    let item = |i: usize| Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * (SIDE_ITEM + 4.0)), vec2(rect.width(), SIDE_ITEM));
    let active = groups.iter().position(|g| *g == shown).unwrap_or(0);
    let lit = anim::toggle(&ctx, id.with("lit"), !searching, Spring::SOFT);
    let y = anim::value(&ctx, id.with("pill"), item(active).top() - rect.top(), Spring::BOUNCY);
    let pill = Rect::from_min_size(pos2(rect.left(), rect.top() + y), vec2(rect.width(), SIDE_ITEM));
    let p = ui.painter();
    p.rect_filled(pill, theme::radius(12.0), theme::white(0.07 * lit));
    p.rect_filled(Rect::from_center_size(pos2(pill.left() + 1.5, pill.center().y), vec2(3.0, 18.0)), theme::radius(1.5), theme::alpha(theme::ACCENT, lit));
    let mut picked = None;
    for (i, group) in groups.iter().enumerate() {
        let r = item(i);
        let resp = ui.interact(r, id.with(("item", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if widgets::pressed(&resp) {
            picked = Some(*group);
        }
        let on = *group == shown && !searching;
        let none = searching && counts.get(i).copied().unwrap_or(0) == 0;
        let target = if on { egui::Color32::WHITE } else if resp.hovered() { theme::FG_2 } else if none { theme::FG_4 } else { theme::FG_3 };
        let fg = anim::color(&ctx, id.with(("fg", i)), target, Spring::SNAPPY);
        let icon_c = anim::color(&ctx, id.with(("ic", i)), if on { theme::ACCENT } else { target }, Spring::SNAPPY);
        let p = ui.painter();
        if on {
            widgets::icon_fill(p, pos2(r.left() + 22.0, r.center().y), group.icon(), 16.0, icon_c);
        } else {
            widgets::icon(p, pos2(r.left() + 22.0, r.center().y), group.icon(), 16.0, icon_c);
        }
        let count = counts.get(i).copied().unwrap_or(0);
        let mut room = r.width() - 46.0 - 12.0;
        // While searching: how many results each has, where it has any.
        if searching && count > 0 {
            let t = Txt::new(count.to_string(), 11.0).color(theme::FG_2);
            let s = widgets::text_size(ui, &t);
            let badge = Rect::from_min_size(pos2(r.right() - 10.0 - s.x - 14.0, r.center().y - 10.0), vec2(s.x + 14.0, 20.0));
            ui.painter().rect_filled(badge, theme::radius(10.0), theme::alpha(theme::ACCENT, 0.16));
            widgets::text(ui.painter(), badge.center(), Align2::CENTER_CENTER, &t);
            room -= badge.width() + 8.0;
        }
        widgets::text_truncated(ui.painter(), pos2(r.left() + 42.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(group.title(), 13.5).weight(weight::MEDIUM).color(fg), room);
    }
    picked
}

/// The sub-tabs in a row over the content, in a narrow window: the one shown with its name, the others by their icon
/// (their name on hover), as the app's page tabs are while a capture shows.
fn tab_row(ui: &mut Ui, rect: Rect, groups: &[Group], shown: Group, searching: bool, counts: &[usize]) -> Option<Group> {
    let ctx = ui.ctx().clone();
    let id = Id::unique("settings-tab-row");
    let gap = 6.0;
    let icon_w = 40.0;
    let others = groups.len().saturating_sub(1) as f32;
    // The shown tab gets what its name needs, up to what the others leave.
    let name = Txt::new(shown.title(), 13.5).weight(weight::MEDIUM);
    let room = (rect.width() - others * (icon_w + gap)).max(icon_w);
    let wanted = (widgets::text_size(ui, &name).x + 58.0).min(room);
    let mut x = rect.left();
    let mut picked = None;
    for (i, group) in groups.iter().enumerate() {
        let on = *group == shown && !searching;
        let w = anim::value(&ctx, id.with(("w", i)), if *group == shown { wanted } else { icon_w }, Spring::SNAPPY);
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        x += w + gap;
        let resp = ui.interact(r, id.with(("tab", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let resp = if *group == shown { resp } else { resp.on_hover_text(group.title()) };
        if widgets::pressed(&resp) {
            picked = Some(*group);
        }
        let hover = anim::toggle(&ctx, id.with(("hover", i)), resp.hovered(), Spring::SNAPPY);
        let lit = anim::toggle(&ctx, id.with(("lit", i)), on, Spring::SOFT);
        let p = ui.painter();
        p.rect_filled(r, theme::radius(12.0), theme::white(0.03 + 0.04 * lit + 0.03 * hover));
        if lit > 0.01 {
            p.rect_filled(Rect::from_center_size(pos2(r.center().x, r.bottom() - 1.5), vec2(18.0, 3.0)), theme::radius(1.5), theme::alpha(theme::ACCENT, lit));
        }
        let c = if on { theme::ACCENT } else if resp.hovered() { theme::FG_2 } else { theme::FG_3 };
        let icon_at = if *group == shown { pos2(r.left() + 20.0, r.center().y) } else { r.center() };
        if on {
            widgets::icon_fill(p, icon_at, group.icon(), 16.0, c);
        } else {
            widgets::icon(p, icon_at, group.icon(), 16.0, c);
        }
        if *group == shown && w > icon_w + 12.0 {
            let fg = if on { egui::Color32::WHITE } else { theme::FG_2 };
            widgets::text_truncated(p, pos2(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(group.title(), 13.5).weight(weight::MEDIUM).color(fg), w - 50.0);
        }
        let count = counts.get(i).copied().unwrap_or(0);
        if searching && count > 0 {
            let dot = pos2(r.right() - 8.0, r.top() + 8.0);
            p.circle_filled(dot, 4.0, theme::ACCENT);
        }
    }
    picked
}

/// The results of a search, best first, in one card: each with its sub-tab and card above its title, what it does
/// under it, the words found marked; the one the arrow keys are on lit. None: what to try instead.
fn results(ui: &mut Ui, cx: &Cx, state: &mut State, found: &[Found], words: &[String], chosen: &mut Option<&'static Entry>, cleared: &mut bool) {
    if found.is_empty() {
        section(ui, cx, 0, icons::MAGNIFYING_GLASS, tr("No matching settings"), Some(tr(NO_RESULTS)), |ui| {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
            if Button::new(tr("Clear the search")).icon(icons::X).variant(Variant::Secondary).size(Size::Sm).show(&mut bu).clicked() {
                *cleared = true;
            }
            ui.add_space(8.0);
        });
        return;
    }
    let title = tr_n("{count} matching setting", "{count} matching settings", found.len() as u64);
    let keys = tr("↑ ↓ to move, Enter to open, Esc to clear");
    let shown = found.len().min(state.limit);
    // The pointer picks the lit result only as it moves: one left over a row never takes it back from the arrows.
    let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
    #[cfg(test)]
    state.result_rects.clear();
    section(ui, cx, 0, icons::MAGNIFYING_GLASS, &title, Some(keys), |ui| {
        ui.add_space(6.0);
        for (i, f) in found.iter().take(shown).enumerate() {
            let needs = hidden_by(f.entry, cx.snap);
            let picked = result_row(ui, f, words, i == state.active, needs);
            #[cfg(test)]
            state.result_rects.push(picked.rect);
            if i == state.active && state.keyed {
                ui.scroll_to_rect(picked.rect, None);
            }
            if picked.hovered && moved {
                state.active = i;
            }
            if picked.pressed {
                *chosen = Some(f.entry);
            }
        }
        state.keyed = false;
        if found.len() > shown {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
            if Button::new(tr("Show more results")).variant(Variant::Ghost).size(Size::Sm).show(&mut bu).clicked() {
                state.limit += RESULTS;
            }
        }
        ui.add_space(6.0);
    });
}

const NO_RESULTS: &str = mark("Try another word, or the name of a feature: microphone, VR, language, update.");

struct RowPick {
    rect: Rect,
    hovered: bool,
    pressed: bool,
}

/// What hides `entry` here, when it is hidden: the setting it needs, and what is actually not so (in English).
fn hidden_by(entry: &Entry, snap: &Snapshot) -> Option<(registry::Needs, &'static str)> {
    entry.needs.and_then(|n| (n.unmet)(snap).map(|when| (n, when)))
}

/// The setting a result or a link brings into view: itself, or, while it is hidden, the setting that shows it (followed
/// up the chain).
pub(super) fn target_of(entry: &'static Entry, snap: &Snapshot) -> &'static Entry {
    let mut at = entry;
    for _ in 0..4 {
        match hidden_by(at, snap).and_then(|(n, _)| registry::entry(n.parent)) {
            Some(parent) => at = parent,
            None => break,
        }
    }
    at
}

/// One result; where it is, whether it is hovered or pressed. A setting hidden until another allows it says so.
fn result_row(ui: &mut Ui, f: &Found, words: &[String], active: bool, needs: Option<(registry::Needs, &'static str)>) -> RowPick {
    let e = f.entry;
    let width = ui.available_width();
    let text_w = width - 28.0 - 32.0;
    let title_text = e.shown(e.title);
    let place = if e.card == e.title { e.group.title().to_string() } else { format!("{} › {}", e.group.title(), e.shown_card()) };
    let crumb = ui.painter().layout_job(marked(&place, words, Txt::new("", 11.5).color(theme::FG_3), text_w, 1));
    let title = ui.painter().layout_job(marked(title_text, words, Txt::new("", 14.0).weight(weight::MEDIUM), text_w, 2));
    let about = (!e.about.is_empty()).then(|| ui.painter().layout_job(marked(tr(e.about), words, Txt::new("", 12.0).color(theme::FG_3).line_height(17.0), text_w, 2)));
    let related = (f.by_keyword && !e.keywords.is_empty())
        .then(|| ui.painter().layout_job(marked(&tr!("Related: {words}", words = tr(e.keywords)), words, Txt::new("", 11.5).color(theme::FG_4), text_w, 1)));
    let hidden = needs.map(|(_, when)| ui.painter().layout_job(Txt::new(format!("↳ {}", tr(when)), 11.5).color(theme::WARN).job(text_w)));
    let h = 12.0
        + crumb.size().y
        + 3.0
        + title.size().y
        + about.as_ref().map_or(0.0, |g| 3.0 + g.size().y)
        + related.as_ref().map_or(0.0, |g| 3.0 + g.size().y)
        + hidden.as_ref().map_or(0.0, |g| 4.0 + g.size().y)
        + 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    if !ui.is_rect_visible(r) {
        return RowPick { rect: r, hovered: false, pressed: false };
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let lit = anim::toggle(ui.ctx(), resp.id.with("lit"), active, Spring::SNAPPY);
    let p = ui.painter();
    if lit > 0.01 {
        p.rect_filled(r, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.10 * lit));
    }
    let mut y = r.top() + 12.0;
    let x = r.left() + 14.0;
    let crumb_h = crumb.size().y;
    p.galley(pos2(x, y), crumb, theme::FG_3);
    y += crumb_h + 3.0;
    let title_h = title.size().y;
    p.galley(pos2(x, y), title, theme::FG);
    y += title_h;
    if let Some(g) = about {
        y += 3.0;
        let gh = g.size().y;
        p.galley(pos2(x, y), g, theme::FG_3);
        y += gh;
    }
    if let Some(g) = related {
        y += 3.0;
        let gh = g.size().y;
        p.galley(pos2(x, y), g, theme::FG_4);
        y += gh;
    }
    if let Some(g) = hidden {
        p.galley(pos2(x, y + 4.0), g, theme::WARN);
    }
    widgets::icon(p, pos2(r.right() - 22.0, r.center().y), icons::ARROW_RIGHT, 14.0, theme::mix(theme::FG_4, theme::ACCENT, lit));
    RowPick { rect: r, hovered: resp.hovered(), pressed: widgets::pressed(&resp) }
}

/// `text` laid out in `look`'s font and colour, the words of the search on a tint of the accent.
fn marked(text: &str, words: &[String], look: Txt, width: f32, rows: usize) -> egui::text::LayoutJob {
    let base = look.format();
    let mut lit = base.clone();
    lit.background = theme::alpha(theme::ACCENT, 0.26);
    lit.color = theme::FG;
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for m in registry::marks(text, words) {
        if m.start > at {
            job.append(&text[at..m.start], 0.0, base.clone());
        }
        job.append(&text[m.clone()], 0.0, lit.clone());
        at = m.end;
    }
    if at < text.len() || text.is_empty() {
        job.append(&text[at..], 0.0, base);
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = rows;
    job
}

/// A sub-tab: its name and what is in it, the setting the search opened, then its cards. Each card and each of its rows
/// is noted as it is drawn: the setting a link asks for is found among them, brought into view and lit.
fn group_page(ui: &mut Ui, cx: &mut Cx, state: &mut State, base: &Settings, draft: &mut Settings, asked: &mut Asked, now: f64) {
    let group = state.group;
    super::block(ui, cx, 0, 48.0, |ui, r| {
        let p = ui.painter();
        widgets::text(p, pos2(r.left() + 2.0, r.top() + 13.0), Align2::LEFT_CENTER, &Txt::new(group.title(), 20.0).weight(weight::SEMIBOLD).tracking(-0.01));
        widgets::text_truncated(p, pos2(r.left() + 2.0, r.top() + 38.0), Align2::LEFT_CENTER, &Txt::new(group.about(), 12.5).color(theme::FG_3), r.width() - 4.0);
    });
    // A link to a setting hidden until another allows it lands on that one.
    if let Some(jump) = state.jump.as_mut().filter(|j| j.lit.is_none()) {
        if let Some(entry) = registry::entry(jump.id) {
            jump.id = target_of(entry, cx.snap).id;
        }
    }
    if let Some(entry) = state.found.and_then(registry::entry).filter(|e| e.group == group) {
        let when = hidden_by(entry, cx.snap).map(|(_, when)| tr(when));
        if found_banner(ui, entry, when) {
            state.found = None;
        }
    }
    let ((cards, rows), controls) = widgets::record_controls(|| widgets::record_fields(|| group_cards(ui, cx, state, base, draft, asked)));
    let card_of = |at: egui::Pos2| cards.iter().find(|(_, c)| c.contains(at)).map_or("", |(n, _)| *n);
    #[cfg(test)]
    {
        state.seen = rows.iter().map(|r| (card_of(r.rect.center()), r.label.clone(), r.rect)).collect();
        state.seen_cards = cards.clone();
        state.seen_fields = rows.iter().map(|r| (r.label.clone(), r.control, r.text_width)).collect();
        state.seen_controls = controls.iter().filter(|c| c.rect.is_positive()).map(|c| (card_of(c.rect.center()), c.key.clone(), c.rect)).collect();
        state.seen_kinds = controls.iter().filter(|c| c.rect.is_positive()).map(|c| c.kind).collect();
    }
    let _ = &card_of;
    light(ui, state, &cards, &rows, &controls, now);
}

/// Above a sub-tab, the setting the search opened in it: where it is, and a cross. True when it is dismissed.
fn found_banner(ui: &mut Ui, entry: &Entry, when: Option<&str>) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(orbiters_ui::layout::SECTION_ALONE_MAX), 44.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.08));
    widgets::icon(p, pos2(r.left() + 20.0, r.center().y), icons::MAGNIFYING_GLASS, 14.0, theme::ACCENT);
    let place = if entry.card == entry.title { entry.group.title().to_string() } else { format!("{} › {}", entry.group.title(), entry.shown_card()) };
    let t = widgets::text_truncated(p, pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(format!("{place} › "), 12.5).color(theme::FG_3), r.width() * 0.5);
    let n = widgets::text_truncated(p, pos2(t.right(), r.center().y), Align2::LEFT_CENTER, &Txt::new(entry.shown(entry.title), 12.5).weight(weight::SEMIBOLD), r.right() - t.right() - 52.0);
    // Hidden until another setting allows it: what it needs, beside it (that setting is the one lit).
    if let Some(when) = when {
        widgets::text_truncated(p, pos2(n.right() + 10.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(when, 12.0).color(theme::WARN), (r.right() - n.right() - 62.0).max(0.0));
    }
    let x = Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(28.0, 28.0));
    widgets::IconButton::new(icons::X, tr("Dismiss")).size(28.0, 13.0).id_salt("settings-found-dismiss").show_at(ui, x).clicked()
}

/// Brings the setting a link asked for into view, the frame its card is drawn, and lights it for a moment: its row on a
/// soft tint of the accent behind its text, or a whole card ringed.
fn light(ui: &mut Ui, state: &mut State, cards: &[(&'static str, Rect)], rows: &[widgets::FieldRow], controls: &[widgets::ControlMark], now: f64) {
    let Some(jump) = state.jump.as_mut() else { return };
    let Some(entry) = registry::entry(jump.id).filter(|e| e.group == state.group) else {
        state.jump = None;
        return;
    };
    let card = cards.iter().find(|(c, r)| *c == entry.card && r.height() > 1.0).map(|(_, r)| *r);
    let row = card.filter(|_| !entry.whole).and_then(|c| rows.iter().find(|r| c.contains(r.rect.center()) && entry.has_row(&r.label)));
    // A setting that is a control of its own (a slider, chips): the controls of it drawn in its card, together.
    let control = card.filter(|_| row.is_none() && !entry.whole).and_then(|c| {
        controls.iter().filter(|m| m.rect.is_positive() && c.contains(m.rect.center()) && entry.controls.contains(&m.key.as_str())).map(|m| m.rect).reduce(|a, b| a.union(b))
    });
    let Some(target) = row.map(|r| r.rect).or(control).or(card) else {
        // Not drawn (yet): a card can come a frame late; after half a second the sub-tab alone is shown.
        jump.waited += 1;
        if jump.waited > 30 {
            state.jump = None;
        }
        return;
    };
    let lit = *jump.lit.get_or_insert_with(|| {
        ui.scroll_to_rect(target, Some(egui::Align::Center));
        now
    });
    let k = glow(now - lit);
    if k <= 0.0 || jump.quiet {
        state.jump = None;
        return;
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    match row {
        Some(row) => {
            let r = row.rect.expand2(vec2(12.0, -2.0));
            let (painter, slot) = &row.behind;
            painter.set(*slot, egui::Shape::rect_filled(r, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.14 * k)));
            ui.painter().rect_stroke(r, theme::radius(12.0), egui::Stroke::new(1.0, theme::alpha(theme::ACCENT, 0.45 * k)), egui::StrokeKind::Inside);
        }
        None if control.is_some() => {
            let r = target.expand(6.0);
            ui.painter().rect_filled(r, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.10 * k));
            ui.painter().rect_stroke(r, theme::radius(12.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.6 * k)), egui::StrokeKind::Outside);
        }
        None => {
            for (grow, a) in [(2.0, 0.8), (6.0, 0.18), (10.0, 0.07)] {
                ui.painter().rect_stroke(target.expand(grow), theme::radius(theme::CARD_RADIUS + grow), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, a * k)), egui::StrokeKind::Outside);
            }
        }
    }
}

/// How lit a setting brought into view is, `t` seconds after: up at once, held, then gone by three seconds.
pub(crate) fn glow(t: f64) -> f32 {
    let t = t as f32;
    if t < 0.18 {
        (t / 0.18).max(0.05)
    } else if t < 2.2 {
        1.0
    } else {
        (1.0 - (t - 2.2) / 0.8).max(0.0)
    }
}

/// Draws card `key` (its English title `card`) in its column, and notes where it went.
fn place(m: &mut super::Masonry, cards: &mut Vec<(&'static str, Rect)>, key: usize, card: &'static str, f: impl FnOnce(&mut Ui) -> bool) -> bool {
    m.add(key, |ui| {
        let r = ui.scope(f);
        cards.push((card, r.response.rect));
        r.inner
    })
}

/// The cards of the sub-tab shown, and where each went.
fn group_cards(ui: &mut Ui, cx: &mut Cx, state: &mut State, base: &Settings, draft: &mut Settings, asked: &mut Asked) -> Vec<(&'static str, Rect)> {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let vrchat_here = orbiters_core::osc::VRCHAT_RUNS_HERE;
    let mut cards = Vec::new();
    let mut m = super::Masonry::new(ui, &format!("settings-{}", state.group.id()));
    let c = &mut cards;
    let mut apply = false;
    match state.group {
        Group::Account => {
            place(&mut m, c, 1, mark("Orbiters account"), |ui| {
                account(ui, cx, &mut state.confirm_sign_out);
                false
            });
            place(&mut m, c, 2, mark("VRChat account"), |ui| {
                vrchat_account(ui, cx, &mut state.vrchat);
                false
            });
        }
        Group::N4n0 => {
            apply |= place(&mut m, c, 1, "N4N0", |ui| super::settings_n4n0::n4n0_card(ui, cx, 1, base, draft, &mut state.texts));
            apply |= place(&mut m, c, 2, mark("N4N0's menu"), |ui| super::settings_n4n0::menu_card(ui, cx, 2, draft));
            apply |= place(&mut m, c, 3, mark("Calls"), |ui| super::settings_n4n0::calls_card(ui, cx, 3, draft));
        }
        Group::Voice => {
            apply |= place(&mut m, c, 1, mark("Assistant"), |ui| super::settings_n4n0::assistant_card(ui, cx, 1, draft));
            apply |= place(&mut m, c, 2, mark("N4N0's ears"), |ui| super::settings_n4n0::ears_card(ui, cx, 2, draft, &mut state.ears_mic));
            // Setting Windows' default is Windows' alone.
            if cfg!(windows) {
                place(&mut m, c, 3, mark("Microphones"), |ui| {
                    super::mic_grid::card(ui, cx, 3, &mut state.mic_grid);
                    false
                });
            }
        }
        Group::Vr => {
            let mut smoothing_reset = false;
            if vrchat_here {
                apply |= place(&mut m, c, 1, registry::CARD_AROUND, |ui| {
                    let mut apply = false;
                    section(ui, cx, 1, icons::STACK, tr(registry::CARD_AROUND), Some(tr("Orbiters beside VRChat's Quick Menu, its wheel and its camera")), |ui| {
                        apply = orbit_settings(ui, snap, &mut draft.orbit, &mut smoothing_reset);
                    });
                    apply
                });
                if smoothing_reset {
                    cx.toasts.push(&ctx, toast::Kind::Info, tr("Smoothing back to the values measured to match VRChat"));
                }
                apply |= place(&mut m, c, 2, registry::CARD_HEADSET, |ui| headset_card(ui, cx, 2, draft));
                apply |= place(&mut m, c, 3, mark("When SteamVR starts"), |ui| super::vr_audio::show(ui, cx, 3, &mut state.vr_audio, &mut draft.vr_audio));
            }
        }
        Group::Capture => {
            apply |= place(&mut m, c, 1, mark("Capture"), |ui| capture_card(ui, cx, 1, base, draft, &mut state.texts));
            apply |= place(&mut m, c, 2, mark("VRChat and OSC"), |ui| osc_card(ui, cx, 2, draft));
            if vrchat_here {
                apply |= place(&mut m, c, 3, mark("Performance"), |ui| performance_card(ui, cx, 3, draft, &mut state.mic));
            }
            apply |= place(&mut m, c, 4, mark("Storage"), |ui| storage_card(ui, cx, 4, &ctx, draft, state, asked));
        }
        Group::Vrchat => {
            if vrchat_here {
                // One control someone comes for, first.
                apply |= place(&mut m, c, 1, mark("FPS booster"), |ui| super::booster_card::card(ui, cx, 1, &mut state.booster, draft));
                // How Home's Play buttons start VRChat.
                apply |= place(&mut m, c, 2, mark("Play VRChat"), |ui| super::launch_card::card(ui, cx, 2, draft));
                place(&mut m, c, 3, mark("Performance report"), |ui| {
                    section(ui, cx, 3, icons::GAUGE, tr("Performance report"), Some(tr("What N4N0 measured, for when VRChat or your computer lags")), |ui| {
                        // A hint's row is named by what it advises: not a setting of its own.
                        widgets::unrecorded(|| super::perf_card::show(ui, cx.ctl, snap));
                    });
                    false
                });
            }
            // Only for an account the server says has it: nobody else sees a trace of it. On a Mac too, where VRChat runs
            // under Obelisk's engine: what it can do there is its capabilities' to say (bridge/caps.rs).
            if snap.bridge.allowed {
                apply |= place(&mut m, c, 4, mark("Experimental"), |ui| super::experimental::show(ui, cx, 4, &mut state.experimental, &mut draft.experimental));
            }
            // A Mac: how Home's Play starts VRChat there (its picture detail).
            #[cfg(target_os = "macos")]
            {
                apply |= place(&mut m, c, 2, mark("Play VRChat"), |ui| super::launch_card::card(ui, cx, 2, draft));
            }
            // What VRChat on this very computer gives (Spout, its log, SteamVR) has no settings where it cannot run.
            if !vrchat_here {
                apply |= place(&mut m, c, 5, mark("VRChat on your network"), |ui| widgets::unrecorded(|| super::network::card(ui, cx, 5, &mut state.network, draft)));
            }
        }
        Group::Language => {
            apply |= place(&mut m, c, 1, mark("Language"), |ui| super::language_card::card(ui, cx, 1, draft));
            if vrchat_here {
                apply |= place(&mut m, c, 2, mark("Translation in VR"), |ui| super::translate_card::card(ui, cx, 2, draft));
            }
        }
        Group::Privacy => {
            if vrchat_here {
                apply |= place(&mut m, c, 1, registry::CARD_PRIVACY, |ui| privacy_card(ui, cx, 1, draft));
                apply |= place(&mut m, c, 2, mark("Beta feedback"), |ui| super::beta_feedback::card(ui, cx, 2, &mut state.beta, draft));
            }
        }
        Group::App => {
            apply |= place(&mut m, c, 1, mark("Interface"), |ui| interface_card(ui, cx, 1, draft));
            place(&mut m, c, 2, mark("Start"), |ui| {
                start_card(ui, cx, 2, state, &ctx);
                false
            });
            apply |= place(&mut m, c, 3, mark("About"), |ui| about_card(ui, cx, 3, draft, state, asked));
            place(&mut m, c, 4, mark("Log"), |ui| {
                let shown = &mut state.log_shown;
                section(ui, cx, 4, icons::SCROLL, tr("Log"), Some(tr("Every notice, warning and error N4N0 showed, kept here")), |ui| notice_log(ui, shown));
                false
            });
        }
    }
    m.finish(ui);
    asked.apply |= apply;
    cards
}

fn capture_card(ui: &mut Ui, cx: &Cx, index: usize, base: &Settings, draft: &mut Settings, texts: &mut Texts) -> bool {
    let snap = cx.snap;
    let mut apply = false;
    section(ui, cx, index, icons::GAUGE, tr("Capture"), Some(tr("How frames become a session")), |ui| {
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            let kf = &mut draft.keyframes;
            widgets::field(ui, tr("Keyframe distance"), Some(tr("A new lossless keyframe after the camera moves this far.")), 284.0, |ui| {
                apply |= widgets::slider(ui, "kf-distance", &mut kf.min_translation, 0.05..=2.0, 0.05, |v| format!("{v:.2} m"));
            });
            widgets::separator(ui);
            widgets::field(ui, tr("Keyframe turn"), Some(tr("…or after it turns this much.")), 284.0, |ui| {
                apply |= widgets::slider(ui, "kf-turn", &mut kf.min_rotation_deg, 2.0..=60.0, 1.0, |v| format!("{v:.0}°"));
            });
            widgets::separator(ui);
            widgets::field(ui, tr("Keyframe interval"), Some(tr("…or after this long, whatever happens.")), 284.0, |ui| {
                apply |= widgets::slider(ui, "kf-interval", &mut kf.max_interval_s, 0.25..=10.0, 0.25, |v| format!("{v:.2} s"));
            });
            widgets::separator(ui);
            widgets::field(ui, tr("New session on world change"), Some(tr("Each VRChat world gets its own session while you record.")), 48.0, |ui| {
                apply |= widgets::toggle(ui, "split", &mut draft.split_on_world_change);
            });
            widgets::separator(ui);
            let placeholder = snap.sources.first().map(|s| s.name.clone()).unwrap_or_else(|| "VRChat".into());
            let (room, field) = form::text_row_width(ui, ui.available_width(), 208.0);
            widgets::field(ui, tr("Spout sender"), Some(tr("Leave empty to pick VRChat's Stream camera automatically.")), room, |ui| {
                if let Some(text) = form::text_row(ui, texts, "spout-sender", &base.spout_sender, &placeholder, field, false) {
                    draft.spout_sender = text;
                    apply = true;
                }
            });
            if !snap.sources.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for s in &snap.sources {
                        if source_chip(ui, "spout-source", &format!("{} · {}×{}", s.name, s.width, s.height), draft.spout_sender == s.name).clicked() {
                            draft.spout_sender = s.name.clone();
                            texts.forget("spout-sender");
                            apply = true;
                        }
                    }
                });
                ui.add_space(8.0);
            }
            widgets::separator(ui);
            let places = [
                SegOption::new(WhilePlaying::GraphicsCard, tr("Graphics card"))
                    .hint(tr("Fastest; in VR it trains in the time VRChat's frames leave idle, on the desktop on at most a third of the card, so slower while the game runs")),
                SegOption::new(WhilePlaying::IntegratedGpu, tr("Integrated GPU")).hint(tr("About 40 times slower, and VRChat keeps its frame rate; once VRChat closes it starts again on the graphics card")),
                SegOption::new(WhilePlaying::AfterVrchat, tr("After VRChat")).hint(tr("Reconstructions wait until VRChat closes or the headset has been off a minute")),
            ];
            widgets::field(
                ui,
                tr("Reconstruct while VRChat runs"),
                Some(tr("On the graphics card training gives way to VRChat's frames; the processor's integrated GPU leaves the game alone but is about 40 times slower.")),
                360.0,
                |ui| {
                    apply |= widgets::segmented(ui, "while-playing", &mut draft.reconstruct_while_playing, &places, true);
                },
            );
            widgets::separator(ui);
        }
        // As wide as its button says it in the language (French's is wider than English's).
        let again = Button::new(tr("Show again")).variant(Variant::Ghost).size(Size::Sm);
        let room = again.desired_width(ui).max(120.0);
        widgets::field(ui, tr("Capture notice"), Some(tr("The consent reminder shown before the first capture.")), room, |ui| {
            if again.show(ui).clicked() {
                draft.consent_acknowledged = false;
                apply = true;
            }
        });
    });
    apply
}

fn osc_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let snap = cx.snap;
    let mut apply = false;
    let osc = &snap.osc;
    let osc_sub = if osc.listening {
        if osc.advertising {
            tr!("Listening on port {port}, advertised through OSCQuery", port = osc.port.unwrap_or(0))
        } else {
            tr!("Listening on port {port}", port = osc.port.unwrap_or(0))
        }
    } else {
        osc.error.clone().unwrap_or_else(|| tr("Off").into())
    };
    section(ui, cx, index, icons::BROADCAST, tr("VRChat and OSC"), Some(&osc_sub), |ui| {
        widgets::field(
            ui,
            tr("Action Menu control"),
            Some(tr("Start and stop recording with N4N0 › Record in your avatar's Action Menu, and let N4N0 and its capture kit hear Orbiters (OSCQuery, works next to VRCFT and other OSC apps).")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "osc", &mut draft.osc.enabled);
            },
        );
        if !orbiters_core::osc::VRCHAT_RUNS_HERE {
            return;
        }
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Stream camera"),
            Some(tr("While recording, open VRChat's camera in Stream mode with Spout Stream on (the picture comes from it), then put it back.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "osc-camera", &mut draft.osc.drive_camera);
            },
        );
        widgets::separator(ui);
        let every = [SegOption::new(RigViews::Off, tr("Off")), SegOption::new(RigViews::Every2s, "2 s"), SegOption::new(RigViews::Every3s, "3 s"), SegOption::new(RigViews::Every5s, "5 s")];
        widgets::field(
            ui,
            tr("Six more views while recording"),
            Some(tr("This often, N4N0's camera rig takes six views around your camera (left, front, right, back, up, down) wherever it has not yet, so the reconstruction sees what you did not film. VRChat draws the world six more times for those moments; 3D pictures and scans take theirs all along.")),
            // As wide as its four small segments, so that the description keeps the row.
            180.0,
            |ui| {
                apply |= widgets::segmented(ui, "rig-views", &mut draft.rig_views, &every, true);
            },
        );
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Camera dolly streams (experimental)"),
            Some(tr("VRC+: a 3D picture flies four cameras with VRChat's camera dolly, one left, one right and one above yours, all looking where yours does, and recordings also read the dolly's other streams. In VRChat, set Camera › Dolly › Streaming to Multi once.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "dolly-streams", &mut draft.dolly_streams);
            },
        );
        widgets::separator(ui);
        let log_desc = snap.vrchat.world.log_file.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| {
            if snap.vrchat.log_found {
                tr("Found, waiting for VRChat").into()
            } else {
                tr("Not found on this machine").into()
            }
        });
        widgets::field(ui, tr("VRChat log"), Some(&log_desc), 120.0, |ui| {
            let (t, c) = if snap.vrchat.log_found { (tr("Read locally"), theme::OK) } else { ("–", theme::FG_4) };
            let (r, _) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::hover());
            widgets::text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(t, 12.0).color(c));
        });
    });
    apply
}

/// What a session records besides the frames: parameters, voice, VRChat's sound, body tracking.
fn performance_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings, mic: &mut MicTest) -> bool {
    let snap = cx.snap;
    let vr = &snap.steamvr;
    let mut apply = false;
    let perf_sub = match (&vr.calibration, vr.running) {
        (Some(c), _) => tr!("Calibrated: eyes at {height} m, hands {span} m apart", height = format!("{:.2}", c.head_height_m), span = format!("{:.2}", c.arm_span_m)),
        _ if snap.vrchat.running => tr("No T-pose needed in VRChat: it sends its head and hand poses").into(),
        (None, true) => tr("SteamVR is running: calibrate in the Orbiters tab of its dashboard").into(),
        (None, false) => tr("What a session records besides the frames").into(),
    };
    section(ui, cx, index, icons::PERSON_ARMS_SPREAD, tr("Performance"), Some(&perf_sub), |ui| {
        widgets::field(
            ui,
            tr("Face and avatar parameters"),
            Some(tr("Every parameter VRChat sends over OSC: face tracking from VRCFaceTracking, gestures, visemes, movement.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "perf-parameters", &mut draft.osc.record_parameters);
            },
        );
        widgets::separator(ui);
        widgets::field(ui, tr("Your voice"), Some(tr("The microphone, for your avatar's visemes in Mixer.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "perf-mic", &mut draft.performance.record_mic);
        });
        if draft.performance.record_mic {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if source_chip(ui, "perf-mic-device", default_mic(), draft.performance.mic_device.is_empty()).clicked() {
                    draft.performance.mic_device.clear();
                    apply = true;
                }
                for m in &snap.mics {
                    if source_chip(ui, "perf-mic-device", &m.name, draft.performance.mic_device == m.id).clicked() {
                        draft.performance.mic_device = m.id.clone();
                        apply = true;
                    }
                }
            });
            ui.add_space(4.0);
            mic_test(ui, mic, &draft.performance.mic_device);
            ui.add_space(12.0);
        }
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("VRChat sound"),
            Some(tr("What VRChat plays, world sounds and other players' voices; nothing else on the PC. Ask the people around you first.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "perf-game", &mut draft.performance.record_game_audio);
            },
        );
        widgets::separator(ui);
        widgets::field(ui, tr("Body tracking"), Some(tr("Headset, controllers and trackers from SteamVR, full-body estimation included.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "perf-tracking", &mut draft.performance.record_tracking);
        });
        widgets::separator(ui);
        widgets::field(ui, tr("Tracking rate"), Some(tr("Device poses recorded per second.")), 284.0, |ui| {
            apply |= widgets::slider(ui, "perf-rate", &mut draft.performance.tracking_rate_hz, 30.0..=120.0, 10.0, |v| format!("{v:.0} Hz"));
        });
    });
    apply
}

/// In the headset: the SteamVR tab, the watch, the coverage while recording, the walk boost.
fn headset_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let vr = &cx.snap.steamvr;
    let mut apply = false;
    section(ui, cx, index, icons::VIRTUAL_REALITY, tr(registry::CARD_HEADSET), Some(tr("What Orbiters adds to SteamVR while you play")), |ui| {
        let tab = match (vr.running, vr.overlay) {
            (true, true) => tr("Orbiters is in SteamVR's dashboard: record and calibrate without leaving VR."),
            (true, false) => tr("SteamVR is running; the tab appears in a moment."),
            (false, _) => tr("Orbiters as a tab in SteamVR's dashboard while SteamVR runs."),
        };
        widgets::field(ui, tr("SteamVR dashboard tab"), Some(tab), 48.0, |ui| {
            apply |= widgets::toggle(ui, "perf-overlay", &mut draft.steamvr_overlay);
        });
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Orbiters watch"),
            Some(tr("The N4N0 mark on a wrist in VR, shown as you turn the wrist to look at it. Point at it with the other hand and pull the trigger once: N4N0's menu opens before you (or on your avatar's N4N0 screen), used with your controllers' lasers while VRChat keeps them; pull again to close it. Grip it to move it. The Orbiters panel floats beside you from the SteamVR tab's Float button.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "floating-watch", &mut draft.floating.watch);
            },
        );
        if draft.floating.watch {
            widgets::separator(ui);
            use orbiters_core::settings::Hand;
            let wrists = [SegOption::new(Hand::Left, tr("Left wrist")), SegOption::new(Hand::Right, tr("Right wrist"))];
            widgets::field(ui, tr("Watch on"), None, 220.0, |ui| {
                apply |= widgets::segmented(ui, "floating-watch-hand", &mut draft.floating.watch_hand, &wrists, true);
            });
        }
        widgets::separator(ui);
        let hands = [SegOption::new(CoverageOverlay::Off, tr("Off")), SegOption::new(CoverageOverlay::LeftHand, tr("Left hand")), SegOption::new(CoverageOverlay::RightHand, tr("Right hand"))];
        widgets::field(
            ui,
            tr("Coverage while recording"),
            Some(tr("In VR, the camera path and what it has covered float above a controller while you record, as in the capture options of Photos & captures. SteamVR shows it in your headset only: never in VRChat's camera or the recording.")),
            300.0,
            |ui| {
                apply |= widgets::segmented(ui, "coverage-overlay", &mut draft.coverage_overlay, &hands, true);
            },
        );
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Walk boost"),
            Some(tr("In VR, your walking stick carries you further, so you walk faster in VRChat. Also in the SteamVR tab and N4N0's menu.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "walk-boost", &mut draft.walk_boost.enabled);
            },
        );
        if draft.walk_boost.enabled {
            widgets::separator(ui);
            let max = orbiters_core::settings::WalkBoostSettings::MAX_EXTRA * 100.0;
            widgets::field(ui, tr("Strength"), Some(tr("How much faster than VRChat's own walking.")), 284.0, |ui| {
                apply |= widgets::slider(ui, "walk-strength", &mut draft.walk_boost.strength, 0.0..=1.0, 0.05, |v| format!("+{:.0} %", v * max));
            });
            widgets::separator(ui);
            widgets::field(ui, tr("Through SteamVR's driver"), Some(tr("Moves every device rather than the play space, so OVR Advanced Settings' drag and turn add up with your walk. Turn it off only if Fast walk misbehaves.")), 48.0, |ui| {
                apply |= widgets::toggle(ui, "walk-through-driver", &mut draft.walk_boost.through_driver);
            });
            // One thing to do, where it is seen: why the better way is not in use, while it is not.
            if let Some(words) = cx.snap.steamvr.walk_path.and_then(crate::state::WalkPath::lesser_words) {
                widgets::separator(ui);
                widgets::field(ui, tr("Fast walk's lesser mode"), Some(tr(words)), 0.0, |_| {});
            }
        }
    });
    apply
}

fn storage_card(ui: &mut Ui, cx: &Cx, index: usize, ctx: &egui::Context, draft: &mut Settings, state: &mut State, asked: &mut Asked) -> bool {
    let st = &cx.snap.storage;
    let mut apply = false;
    let held = fill(&tr_n("{size} in {count} session", "{size} in {count} sessions", st.sessions as u64), &[("size", &format::bytes(st.used_bytes as f64))]);
    let storage_sub = match st.free_bytes {
        Some(f) => tr!("{held} · {size} free on the drive", held = held, size = format::bytes(f as f64)),
        None => held,
    };
    section(ui, cx, index, icons::HARD_DRIVE, tr("Storage"), Some(&storage_sub), |ui| {
        let used_gb = st.used_bytes as f64 / 1e9;
        let limit = draft.storage_limit_gb;
        let usage = if limit > 0.0 { (used_gb / limit).min(1.0) as f32 } else { 0.0 };
        ui.add_space(12.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let p = ui.painter();
        widgets::text(p, row.left_center(), Align2::LEFT_CENTER, &Txt::new(tr("Library usage"), 12.0).color(theme::FG_3));
        let lim = if limit > 0.0 { format!("{limit:.0} GB") } else { tr("unlimited").into() };
        widgets::text(p, row.right_center(), Align2::RIGHT_CENTER, &Txt::new(format!("{used_gb:.1} / {lim}"), 12.0).color(theme::FG_3));
        ui.add_space(8.0);
        widgets::progress(ui, Id::unique("storage-usage"), usage, 10.0);
        ui.add_space(12.0);
        widgets::separator(ui);
        let can_go = if st.capture_bytes > 0 {
            fill(
                &tr_n(
                    "The capture files of {count} reconstructed capture take {size}: only reconstructing again needs them, its 3D view stays.",
                    "The capture files of {count} reconstructed captures take {size}: only reconstructing again needs them, their 3D views stay.",
                    st.capture_sessions as u64,
                ),
                &[("size", &format::bytes(st.capture_bytes as f64))],
            )
        } else {
            tr("What the captures and 3D pictures take, and deleting what they no longer need.").to_string()
        };
        widgets::field(ui, tr("Free up space"), Some(&can_go), 160.0, |ui| {
            if Button::new(tr("Free up space")).icon(icons::BROOM).size(Size::Sm).show(ui).clicked() {
                asked.free_space = true;
            }
        });
        widgets::separator(ui);
        widgets::field(ui, tr("Storage limit"), Some(tr("Recording stops when the library reaches it (0 = no limit).")), 284.0, |ui| {
            let mut v = draft.storage_limit_gb as f32;
            if widgets::slider(ui, "limit", &mut v, 0.0..=2000.0, 10.0, |v| if v == 0.0 { tr("No limit").into() } else { format!("{v:.0} GB") }) {
                draft.storage_limit_gb = v as f64;
                apply = true;
            }
        });
        widgets::separator(ui);
        if folder_field(ui, tr("Captures folder"), None, &draft.sessions_dir) {
            state.picker = Some((Folder::Sessions, pick(ctx, &draft.sessions_dir)));
        }
        widgets::separator(ui);
        if folder_field(ui, tr("Raw captures folder"), Some(tr("Where Mixer's capture simulator writes.")), &draft.raw_captures_dir) {
            state.picker = Some((Folder::Raw, pick(ctx, &draft.raw_captures_dir)));
        }
    });
    apply
}

/// What N4N0 shares of how VRChat runs, and of how the app is used.
fn privacy_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut apply = false;
    section(ui, cx, index, icons::SHIELD_CHECK, tr(registry::CARD_PRIVACY), Some(tr("What leaves this computer, and only when it is on")), |ui| {
        widgets::field(
            ui,
            tr("Share my frame rate per world"),
            Some(tr("In VR, a summary a minute: the world, your FPS, your avatar, how many people were there and the names of their avatars, never who they are, plus your graphics card, processor and headset. It shows which worlds run well on the Map explorer.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "world-performance", &mut draft.world_performance);
            },
        );
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Beta usage statistics"),
            Some(tr("Counts which pages and buttons of Orbiters - N4N0 you use, in what order and when, and every few seconds how much processor, graphics card and memory the app, VRChat and the whole computer use, VRChat's frame rate in VR, and the basic hardware (processor and graphics card models, memory size, headset and its refresh rate). When you are signed in, it is linked to your Orbiters account and deleted with it. Only the Orbiters team sees it, to make the app easier and lighter; it is never shared publicly. Never what you type or say, pictures, links, worlds or people. Turned off, nothing more is counted and what was waiting to be sent is deleted.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "usage-statistics", &mut draft.usage.enabled);
            },
        );
    });
    apply
}

fn interface_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut apply = false;
    section(ui, cx, index, icons::SPARKLE, tr("Interface"), None, |ui| {
        widgets::field(ui, tr("Reduce motion"), Some(tr("Calmer transitions and a still background.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "reduce-motion", &mut draft.reduce_motion);
        });
        // Minimised for long while VRChat runs, the window closes to the tray after a countdown (window_rest).
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            widgets::separator(ui);
            widgets::field(ui, crate::window_rest::setting_label(), Some(crate::window_rest::setting_hint()), 48.0, |ui| {
                apply |= widgets::toggle(ui, "close-minimized-window", &mut draft.close_minimized_window);
            });
        }
        if let Some(gpu) = cx.gpu.map(|g| &g.adapter).filter(|a| a.choice) {
            widgets::separator(ui);
            let gpus = [
                SegOption::new(WindowGpu::IntegratedGpu, tr("Integrated GPU")).hint(tr("The processor's GPU draws; the graphics card copies each frame to show it")),
                SegOption::new(WindowGpu::GraphicsCard, tr("Graphics card")).hint(tr("Costs a game less than copying the frames from the integrated GPU")),
            ];
            let now = if (draft.window_gpu == WindowGpu::IntegratedGpu) == gpu.integrated {
                tr!("Now on the {name}.", name = gpu.name)
            } else {
                tr!("On the {name} until the window opens again.", name = gpu.name)
            };
            let note = tr!("The graphics card draws the window for less than it costs to copy it there from the integrated GPU. {now}", now = now);
            widgets::field(ui, tr("Window graphics"), Some(&note), 260.0, |ui| {
                apply |= widgets::segmented(ui, "window-gpu", &mut draft.window_gpu, &gpus, true);
            });
            if !gpu.integrated {
                widgets::separator(ui);
                let views = [SegOption::new(ViewGpu::WhilePlaying, tr("While VRChat runs")), SegOption::new(ViewGpu::Always, tr("Always")), SegOption::new(ViewGpu::Never, tr("Never"))];
                let note = tr("The 3D view's splats draw on the integrated GPU and only their pictures reach the graphics card: orbiting costs a game little, at about half the frame rate.");
                widgets::field(ui, tr("3D view on the integrated GPU"), Some(note), 340.0, |ui| {
                    apply |= widgets::segmented(ui, "view-gpu", &mut draft.view_gpu, &views, true);
                });
            }
        }
    });
    apply
}

fn start_card(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State, ctx: &egui::Context) {
    let snap = cx.snap;
    let vrchat_here = orbiters_core::osc::VRCHAT_RUNS_HERE;
    // Each a whole sentence: the tray is called otherwise on a Mac.
    let start_sub = if cfg!(target_os = "macos") { tr("Orbiters - N4N0 opening on its own, in the menu bar") } else { tr("Orbiters - N4N0 opening on its own, in the tray") };
    section(ui, cx, index, icons::POWER, tr("Start"), Some(start_sub), |ui| {
        let mut windows = state.windows_start.unwrap_or(false);
        let (start_label, start_note) = if vrchat_here {
            (tr("Start with Windows"), tr("Waits in the tray, ready when VRChat starts."))
        } else {
            (tr("Start at login"), tr("Waits in the menu bar, ready to talk with VRChat on your network."))
        };
        widgets::field(ui, start_label, Some(start_note), 48.0, |ui| {
            if widgets::toggle(ui, "start-windows", &mut windows) && state.windows_job.is_none() {
                state.windows_start = Some(windows);
                state.windows_job = Some(Job::spawn(ctx, move || crate::startup::set_start_with_windows(windows).map_err(|e| e.to_string())));
            }
        });
        if !vrchat_here {
            return;
        }
        widgets::separator(ui);
        let startup = &snap.settings.startup;
        let mut steamvr = state.steamvr_shown.unwrap_or(startup.steamvr);
        let note = match (startup.steamvr_pending && state.steamvr_job.is_none(), startup.steamvr) {
            (true, true) => tr("SteamVR is not running: it is told at its next start, while Orbiters - N4N0 runs."),
            (true, false) => tr("SteamVR is not running: it stops at its next start, while Orbiters - N4N0 runs."),
            _ => tr("Opens when SteamVR starts, in its dashboard (SteamVR lists it with its startup overlay apps)."),
        };
        widgets::field(ui, tr("Start with SteamVR"), Some(note), 48.0, |ui| {
            if widgets::toggle(ui, "start-steamvr", &mut steamvr) && state.steamvr_job.is_none() {
                state.steamvr_shown = Some(steamvr);
                state.steamvr_job = Some((steamvr, Job::spawn(ctx, move || crate::startup::set_start_with_steamvr(steamvr))));
            }
        });
    });
}

fn about_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings, state: &mut State, asked: &mut Asked) -> bool {
    let snap = cx.snap;
    let mut apply = false;
    section(ui, cx, index, icons::INFO, tr("About"), None, |ui| {
        ui.add_space(8.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let lr = Rect::from_min_size(pos2(row.left(), row.center().y - 21.0), orbiters_ui::n4n0_logo::size_for_width(48.0));
        orbiters_ui::n4n0_logo::paint(ui.painter(), lr, theme::MARK);
        let x = lr.right() + 16.0;
        let p = ui.painter();
        widgets::text(p, pos2(x, row.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(format!("Orbiters - N4N0 {}", snap.version), 15.0).weight(weight::SEMIBOLD));
        widgets::text_truncated(
            p,
            pos2(x, row.center().y + 10.0),
            Align2::LEFT_CENTER,
            &Txt::new(tr("Copyright © 2026 Enzo DUTRA / blackorbit. Free to use; see the licence for what is allowed."), 12.5).color(theme::FG_3),
            row.width() - (x - row.left()) - 240.0,
        );
        if let Some(err) = &snap.worker.error {
            widgets::text_truncated(p, pos2(x, row.center().y + 28.0), Align2::LEFT_CENTER, &Txt::new(err, 12.0).color(theme::DANGER), row.width() - 300.0);
        }
        let mut bu = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(egui::Align::Center)));
        bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
        if cx.windowed && Button::new(tr("Quit")).icon(icons::SIGN_OUT).variant(Variant::Danger).size(Size::Sm).show(&mut bu).clicked() {
            asked.quit = true;
        }
        if Button::new(tr("Worker log")).icon(icons::SCROLL).size(Size::Sm).show(&mut bu).clicked() {
            asked.want_log = true;
        }
        widgets::separator(ui);
        updates(ui, cx.ctl, snap);
        widgets::separator(ui);
        let note = if draft.development {
            tr("On: the Orbiters server running on this computer, its account and its builds of Orbiters - N4N0 as updates.")
        } else {
            tr("For testing development builds: the Orbiters server running on this computer instead of orbiters.cc, with its own account and updates.")
        };
        widgets::field(ui, tr("Development environment"), Some(note), 48.0, |ui| {
            apply |= widgets::toggle(ui, "development", &mut draft.development);
        });
        if state.log_open {
            // The worker's last lines; older ones a press of Show more away, in the page's one scroll.
            let lines = cx.ctl.worker_log().unwrap_or_default();
            state.worker_shown = state.worker_shown.max(WORKER_LINES);
            if lines.len() > state.worker_shown {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
                let more = tr!("Show more ({count} more)", count = lines.len() - state.worker_shown);
                if Button::new(&more).variant(Variant::Ghost).size(Size::Sm).id_salt("worker-log-more").show(&mut bu).clicked() {
                    state.worker_shown += 100;
                }
                ui.add_space(6.0);
            }
            let k = anim::spring_from(ui.ctx(), Id::unique("worker-log-open"), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
            let text = if lines.is_empty() { tr("No output yet.").to_string() } else { lines[lines.len().saturating_sub(state.worker_shown)..].join("\n") };
            let w = ui.available_width();
            let g = ui.painter().layout_job(Txt::new(text, 11.0).mono().color(theme::FG_2).line_height(17.0).job(w - 24.0));
            let (r, _) = ui.allocate_exact_size(vec2(w, (g.size().y + 24.0) * k), Sense::hover());
            ui.painter().rect_filled(r, theme::radius(12.0), theme::INK_900);
            let clip = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
            clip.galley(r.min + vec2(12.0, 12.0), g, theme::FG_2);
            ui.add_space(12.0);
        }
    });
    apply
}

/// The worker log's last lines shown at first under About.
const WORKER_LINES: usize = 40;

/// Updates of an installed app: where they stand, and the button that moves them on (controller/update.rs).
fn updates(ui: &mut Ui, ctl: &Ctl, snap: &Snapshot) {
    let u = &snap.update;
    if !u.enabled {
        widgets::field(ui, tr("Updates"), Some(tr("This copy was not installed by Orbiters - N4N0's setup: it does not update itself.")), 0.0, |_| {});
        return;
    }
    let busy = snap.busy_with();
    let text = match &u.state {
        UpdateState::Idle => match u.checked {
            Some(at) => tr!("Up to date (looked {time}).", time = super::local_time_unix(at)),
            None => tr("Orbiters - N4N0 looks for a newer version shortly after it starts, then every few hours.").into(),
        },
        UpdateState::Checking => tr("Looking for a newer version…").into(),
        UpdateState::Downloading { version, fraction } => tr!("Downloading version {version}: {percent} %", version = version, percent = format!("{:.0}", fraction * 100.0)),
        UpdateState::Ready { version, .. } if u.restart_when_idle => {
            tr!("Version {version} is ready. Orbiters - N4N0 restarts to install it once {work} ends.", version = version, work = tr(busy.unwrap_or(mark("the running work"))))
        }
        UpdateState::Ready { version, .. } => match busy {
            Some(work) => tr!("Version {version} is ready. Restarting now would stop {work}: it can wait for it to end.", version = version, work = tr(work)),
            None => tr!("Version {version} is ready: a restart installs it, and Orbiters - N4N0 opens again.", version = version),
        },
        UpdateState::Failed { message } => message.clone(),
    };
    let hint = busy.map(|work| tr!("Restarts once {work} ends", work = tr(work))).unwrap_or_default();
    widgets::field(ui, tr("Updates"), Some(&text), 300.0, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        match &u.state {
            UpdateState::Ready { .. } if u.restart_when_idle => {
                if Button::new(tr("Restart now")).variant(Variant::Danger).size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::Restart { now: true });
                }
                if Button::new(tr("Not after all")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::CancelRestart);
                }
            }
            UpdateState::Ready { .. } => {
                let label = if busy.is_some() { tr("Restart when done") } else { tr("Restart to update") };
                let mut button = Button::new(label).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary).size(Size::Sm);
                if busy.is_some() {
                    button = button.hint(&hint);
                }
                if button.show(ui).clicked() {
                    ctl.updates(UpdateAction::Restart { now: false });
                }
            }
            UpdateState::Checking | UpdateState::Downloading { .. } => {
                Button::new(tr("Look now")).size(Size::Sm).enabled(false).show(ui);
            }
            UpdateState::Idle | UpdateState::Failed { .. } => {
                if Button::new(tr("Look now")).size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::Check);
                }
            }
        }
    });
    if let UpdateState::Ready { notes, .. } = &u.state {
        let notes = notes.trim();
        if !notes.is_empty() {
            let w = ui.available_width();
            let mut job = Txt::new(notes, 12.0).color(theme::FG_3).line_height(18.0).job(w);
            job.wrap.max_rows = 8;
            let g = ui.painter().layout_job(job);
            let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y + 16.0), Sense::hover());
            ui.painter().galley(r.min, g, theme::FG_3);
        }
    }
}

fn pick(ctx: &egui::Context, current: &std::path::Path) -> Job<Option<PathBuf>> {
    let start = current.to_path_buf();
    Job::spawn(ctx, move || rfd::FileDialog::new().set_directory(&start).pick_folder())
}

/// The Orbiters account the Unity tools share: who is signed in, or the browser sign-in under way.
fn account(ui: &mut Ui, cx: &Cx, confirm_sign_out: &mut bool) {
    let status = &cx.snap.account;
    // The states that are about this computer's own server say so after a dot.
    let (sub, on_server) = match &status.state {
        AccountState::SignedIn { username } => (tr!("Signed in as {username}", username = username), true),
        AccountState::Checking { username: Some(username) } => (tr!("Checking {username}…", username = username), false),
        AccountState::Checking { username: None } => (tr("Checking your sign-in…").to_string(), false),
        AccountState::Offline { username: Some(username) } => (tr!("Signed in as {username} · Orbiters could not be reached", username = username), false),
        AccountState::Offline { username: None } => (tr("Signed in as you · Orbiters could not be reached").to_string(), false),
        AccountState::Expired { .. } => (tr("Your sign-in expired").to_string(), false),
        AccountState::SigningIn => (tr("Finish in your browser").to_string(), true),
        AccountState::SignedOut | AccountState::Unknown => (tr("Not signed in").to_string(), true),
    };
    let sub = if on_server && status.development { tr!("{status} · development server", status = sub) } else { sub };
    if !matches!(status.state, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. }) {
        *confirm_sign_out = false;
    }
    section(ui, cx, 1, icons::USER_CIRCLE, tr("Orbiters account"), Some(&sub), |ui| {
        match &status.state {
            AccountState::SigningIn => {
                let desc = tr("The Orbiters page opened in your browser: sign in there if it asks, then click Connect. Nothing to type here.");
                widgets::field(ui, tr("Connect in your browser"), Some(desc), 240.0, |ui| {
                    if Button::new(tr("Cancel")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.cancel_sign_in();
                    }
                    ui.add_space(8.0);
                    if Button::new(tr("Open again")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.reopen_sign_in();
                    }
                });
            }
            AccountState::SignedOut | AccountState::Expired { .. } | AccountState::Unknown => {
                let desc = tr("One sign-in for Orbiters - N4N0 and your Unity tools: MCB, My Avatar, ReFit and the Logger. Uploads to the Map explorer use it.");
                widgets::field(ui, tr("Sign in"), Some(desc), 140.0, |ui| {
                    if Button::new(tr("Sign in")).icon(icons::SIGN_IN).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.sign_in();
                    }
                });
            }
            _ => {
                let desc = tr("The account MCB, My Avatar and your other Orbiters tools use: signing in or out in one of them does it here too.");
                widgets::field(ui, tr("Shared with your Unity tools"), Some(desc), 300.0, |ui| {
                    if *confirm_sign_out {
                        if Button::new(tr("Cancel")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            *confirm_sign_out = false;
                        }
                        ui.add_space(8.0);
                        if Button::new(tr("Sign out of every tool")).icon(icons::SIGN_OUT).variant(Variant::Danger).size(Size::Sm).show(ui).clicked() {
                            *confirm_sign_out = false;
                            cx.ctl.sign_out();
                        }
                    } else if Button::new(tr("Sign out")).icon(icons::SIGN_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        *confirm_sign_out = true;
                    }
                });
            }
        }
        explorer(ui, cx);
        if let Some(error) = &status.error {
            ui.add_space(4.0);
            let g = ui.painter().layout_job(Txt::new(error, 12.0).color(theme::DANGER).job(ui.available_width()));
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, theme::DANGER);
            ui.add_space(8.0);
        }
    });
}

/// Explorer points (REQUEST13), small, under the account: where the member stands (points, title, what the next title
/// takes, a thin line of the way to it), and the switch for the notice in worlds with no public 3D picture yet.
fn explorer(ui: &mut Ui, cx: &Cx) {
    use crate::controller::explorer as ex;
    let signed_in = matches!(cx.snap.account.state, AccountState::SignedIn { .. } | AccountState::Offline { username: Some(_) });
    if signed_in {
        widgets::separator(ui);
        let desc = match cx.snap.explorer.points.as_ref() {
            Some(points) => ex::standing_words(points),
            None => tr("Shown once Orbiters has said them").to_string(),
        };
        let title = cx.snap.explorer.points.as_ref().and_then(|p| p.title.as_ref()).map(ex::title_name);
        widgets::field(ui, tr(ex::EXPLORER_POINTS), Some(&desc), 160.0, |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(160.0, 24.0), Sense::hover());
            let p = ui.painter();
            if let Some(title) = title.as_ref() {
                widgets::text(p, pos2(r.right(), r.top() + 4.0), Align2::RIGHT_CENTER, &Txt::new(title, 12.5).weight(weight::SEMIBOLD).color(theme::ACCENT));
            }
            if let Some(points) = cx.snap.explorer.points.as_ref() {
                let track = Rect::from_min_max(pos2(r.left(), r.bottom() - 5.0), pos2(r.right(), r.bottom() - 1.0));
                p.rect_filled(track, theme::radius(2.0), theme::white(0.10));
                p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * points.progress(), track.height())), theme::radius(2.0), theme::ACCENT);
            }
        });
    }
    widgets::separator(ui);
    let mut on = cx.snap.settings.explorer.world_notices;
    widgets::field(ui, tr(ex::NOTICES_SWITCH), Some(tr(ex::NOTICES_SWITCH_WHY)), 48.0, |ui| {
        if widgets::toggle(ui, "explorer-world-notices", &mut on) {
            cx.ctl.patch_settings(serde_json::json!({ "explorer": { "world_notices": on } }));
        }
    });
}

/// Secrets are transient UI state, never part of Settings or the snapshot.
#[derive(Default)]
struct VrchatForm {
    username: String,
    password: String,
    code: String,
    method: Option<orbiters_core::vrchat_api::TwoFactor>,
    /// The password shown as typed (its eye).
    show_password: bool,
    pending: bool,
    pending_revision: u64,
    saw_busy: bool,
    picture: Option<(String, u64, egui::TextureHandle)>,
}

impl State {
    /// Synthetic offline data for the screenshot runner; never uses the person's account or credential store.
    pub(crate) fn stage_vrchat(&mut self, entering: bool) {
        if entering {
            self.vrchat.username = "DesktopTester".into();
            self.vrchat.password = "screenshot-only".into();
        }
    }
}

fn vrchat_pressed(response: egui::Response) -> bool {
    response.enabled() && (response.clicked() || (response.is_pointer_button_down_on() && response.ctx.input(|i| i.pointer.primary_pressed())))
}

/// The same flat well as widgets::text_field, with egui's password masking and an eye that shows what was typed.
fn vrchat_password(ui: &mut Ui, password: &mut String, width: f32, shown: &mut bool) -> egui::Response {
    let id = ui.scope_id().with("vrchat-password");
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, theme::radius(12.0), theme::INK_750);
    let ring = anim::toggle(ui.ctx(), id.with("ring"), ui.memory(|m| m.has_focus(id)), Spring::SNAPPY);
    if ring > 0.01 {
        ui.painter().rect_stroke(rect, theme::radius(12.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.8 * ring)), egui::StrokeKind::Outside);
    }
    let eye = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
    let label = if *shown { tr("Hide the password") } else { tr("Show the password") };
    if vrchat_pressed(
        widgets::IconButton::new(if *shown { icons::EYE_SLASH } else { icons::KEY }, label).size(28.0, 14.0).id_salt("vrchat-eye").show_at(ui, eye),
    ) {
        *shown = !*shown;
    }
    ui.put(
        Rect::from_min_max(pos2(rect.left() + 12.0, rect.top()), pos2(eye.left() - 4.0, rect.bottom())),
        egui::TextEdit::singleline(password)
            .id(id)
            .password(!*shown)
            .frame(egui::Frame::NONE)
            .font(theme::font(13.0, weight::REGULAR))
            .text_color(theme::FG)
            .hint_text(egui::RichText::new(tr("VRChat password")).color(theme::FG_4))
            .margin(egui::Margin::ZERO)
            .align(Align2::LEFT_CENTER),
    )
}

/// Six cells for a six-digit code (an authenticator's, an email's): digits only, each popping in as it is typed, the
/// next cell ringed in green while the field has the keys.
fn code_cells(ui: &mut Ui, code: &mut String) -> egui::Response {
    let id = ui.scope_id().with("vrchat-code-cells");
    let (cell, gap, h) = (44.0, 8.0, 50.0);
    let w = 6.0 * cell + 5.0 * gap;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    // The text edit takes the keys (and clicks) over the cells; its own text, caret and selection stay unseen.
    let resp = {
        let mut eu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("vrchat-code-edit"));
        let v = eu.visuals_mut();
        v.text_cursor.stroke = egui::Stroke::NONE;
        v.selection.bg_fill = egui::Color32::TRANSPARENT;
        v.selection.stroke = egui::Stroke::NONE;
        eu.put(
            rect,
            egui::TextEdit::singleline(code)
                .id(id)
                .frame(egui::Frame::NONE)
                .text_color(egui::Color32::TRANSPARENT)
                .font(theme::font(13.0, weight::REGULAR))
                .char_limit(6)
                .margin(egui::Margin::ZERO)
                .desired_width(w),
        )
    };
    code.retain(|c| c.is_ascii_digit());
    let ctx = ui.ctx().clone();
    let focused = ui.memory(|m| m.has_focus(id));
    let n = code.chars().count();
    let p = ui.painter();
    for i in 0..6 {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (cell + gap), rect.top()), vec2(cell, h));
        let next = focused && i == n.min(5);
        let ring = anim::toggle(&ctx, id.with(("ring", i)), next, Spring::SNAPPY);
        let filled = anim::toggle(&ctx, id.with(("filled", i)), i < n, Spring::SOFT);
        p.rect_filled(r, theme::radius(12.0), theme::mix(theme::INK_750, theme::INK_700, filled));
        if ring > 0.01 {
            p.rect_stroke(r, theme::radius(12.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.85 * ring)), egui::StrokeKind::Outside);
        }
        if let Some(c) = code.chars().nth(i) {
            let k = anim::spring_from(&ctx, id.with(("digit", i, c)), [0.55], [1.0], Spring::BOUNCY)[0];
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 22.0 * k.clamp(0.4, 1.3)).weight(weight::SEMIBOLD));
        } else if next && anim::is_live(&ctx) {
            // A caret that blinks where the next digit goes.
            let t = anim::time(&ctx);
            if (t * 1.6).fract() < 0.6 {
                p.rect_filled(Rect::from_center_size(r.center(), vec2(2.0, 20.0)), theme::radius(1.0), theme::FG_2);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::Text)
}

/// The signed-in account: picture with a presence ring, name, how it is kept; Sign out on the right.
fn vrchat_identity(ui: &mut Ui, name: &str, picture: Option<&egui::TextureHandle>, busy: bool) -> bool {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
    let p = ui.painter().clone();
    let a = Rect::from_center_size(pos2(row.left() + 28.0, row.center().y), vec2(52.0, 52.0));
    p.circle_stroke(a.center(), 29.0, egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.9)));
    widgets::avatar(&p, a, picture, name);
    let dot = a.center() + vec2(19.0, 19.0);
    p.circle_filled(dot, 7.0, theme::surface_strong());
    p.circle_filled(dot, 5.0, theme::ACCENT);
    let x = a.right() + 18.0;
    widgets::text_truncated(&p, pos2(x, row.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(name, 16.0).weight(weight::SEMIBOLD), w - 220.0);
    widgets::icon(&p, pos2(x + 6.0, row.center().y + 12.0), icons::LOCK_SIMPLE, 12.0, theme::FG_3);
    widgets::text_truncated(
        &p,
        pos2(x + 18.0, row.center().y + 12.0),
        Align2::LEFT_CENTER,
        &Txt::new(tr("Connected · the session stays encrypted on this computer"), 12.0).color(theme::FG_3),
        w - 240.0,
    );
    let b = Button::new(if busy { tr("Signing out…") } else { tr("Sign out") }).icon(icons::SIGN_OUT).variant(Variant::Secondary).size(Size::Sm).enabled(!busy);
    let bw = b.desired_width(ui);
    vrchat_pressed(b.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.center().y - 16.0), vec2(bw, 32.0))))
}

fn vrchat_account(ui: &mut Ui, cx: &Cx, state: &mut VrchatForm) {
    use crate::controller::vrchat_login::{Command, Recovery};
    use orbiters_core::vrchat_api::{Secret, TwoFactor};
    let status = &cx.snap.vrchat_login;
    // Orbiters being checked or out of reach keeps the card (r20): the VRChat session waits, it is not signed out.
    let orbiters = matches!(cx.snap.account.state, AccountState::SignedIn { .. } | AccountState::Checking { .. } | AccountState::Offline { .. });
    // r21: a saved session being restored shows too, connecting, never as a sign-in form.
    let connecting = status.presence() == crate::controller::vrchat_login::Presence::Connecting && status.methods.is_empty();
    if !(status.allowed || status.waiting || connecting) || !orbiters {
        *state = VrchatForm::default();
        return;
    }
    if status.busy {
        state.saw_busy = true;
    }
    if (state.saw_busy && !status.busy) || (state.pending && state.pending_revision != status.revision) {
        state.pending = false;
        state.saw_busy = false;
    }
    let busy = status.busy || state.pending;
    let subtitle = if let Some(user) = &status.user {
        tr!("Signed in as {username}", username = user.display_name)
    } else if let (true, Some(user)) = (status.waiting, &status.known) {
        tr!("{username}, waiting for Orbiters", username = user.display_name)
    } else if let (true, Some(user)) = (connecting, &status.known) {
        tr!("{username}, connecting…", username = user.display_name)
    } else if !status.methods.is_empty() {
        tr("Two-factor authentication: one more step").into()
    } else {
        tr("Your own account, kept on this computer").into()
    };
    let mut command = None;
    section(ui, cx, 2, icons::VIRTUAL_REALITY, tr("VRChat account"), Some(&subtitle), |ui| {
        ui.add_space(6.0);
        if status.waiting {
            state.picture = None;
            ui.add_space(6.0);
            super::sharing::working(ui, tr("Kept on this computer. It comes back by itself once Orbiters answers."));
        } else if connecting {
            state.picture = None;
            ui.add_space(6.0);
            super::sharing::working(ui, tr("Connecting to VRChat…"));
        } else if let Some(user) = &status.user {
            if let Some(picture) = &status.picture {
                if state.picture.as_ref().is_none_or(|(id, revision, _)| id != &user.id || *revision != status.revision) {
                    let image = egui::ColorImage::from_rgba_unmultiplied([picture.width as usize, picture.height as usize], &picture.rgba);
                    let texture = ui.ctx().load_texture(format!("vrchat-account-{}", user.id), image, egui::TextureOptions::LINEAR);
                    state.picture = Some((user.id.clone(), status.revision, texture));
                }
            } else {
                state.picture = None;
            }
            if vrchat_identity(ui, &user.display_name, state.picture.as_ref().map(|(_, _, t)| t), busy) {
                command = Some(Command::SignOut);
            }
        } else {
            state.picture = None;
            ui.add_enabled_ui(!busy, |ui| {
                if status.methods.is_empty() {
                    widgets::field(ui, tr("Username"), Some(tr("Your VRChat login name or email, not your display name.")), 300.0, |ui| {
                        widgets::text_field(ui, "vrchat-username", &mut state.username, tr("VRChat username or email"), 300.0, false);
                    });
                    widgets::separator(ui);
                    let mut enter = false;
                    widgets::field(ui, tr("Password"), Some(tr("Sent to VRChat only, never to Orbiters.")), 300.0, |ui| {
                        let response = vrchat_password(ui, &mut state.password, 300.0, &mut state.show_password);
                        enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    widgets::separator(ui);
                    ui.add_space(14.0);
                    let ready = !state.username.trim().is_empty() && !state.password.is_empty();
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let note = tr("Instead of Orbiters' shared account: friends, instances and pictures come from yours.");
                    widgets::icon(ui.painter(), pos2(row.left() + 7.0, row.center().y), icons::SHIELD_CHECK, 14.0, theme::FG_3);
                    widgets::text_truncated(
                        ui.painter(),
                        pos2(row.left() + 22.0, row.center().y),
                        Align2::LEFT_CENTER,
                        &Txt::new(note, 12.0).color(theme::FG_3),
                        row.width() - 230.0,
                    );
                    let b = Button::new(if busy { tr("Signing in…") } else { tr("Sign in to VRChat") }).icon(icons::SIGN_IN).variant(Variant::Primary).enabled(ready);
                    let bw = b.desired_width(ui);
                    if (vrchat_pressed(b.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 40.0)))) || enter) && ready {
                        command = Some(Command::SignIn { username: state.username.clone(), password: Secret(std::mem::take(&mut state.password)) });
                    }
                } else {
                    let method = state.method.get_or_insert(status.methods[0]);
                    if !status.methods.contains(method) {
                        *method = status.methods[0];
                    }
                    if status.methods.len() > 1 {
                        let before = *method;
                        let options: Vec<_> = status
                            .methods
                            .iter()
                            .map(|m| {
                                SegOption::new(
                                    *m,
                                    match m {
                                        TwoFactor::Totp => tr("Authenticator app"),
                                        TwoFactor::Email => tr("Email"),
                                        TwoFactor::Recovery => tr("Recovery code"),
                                    },
                                )
                            })
                            .collect();
                        widgets::segmented(ui, "vrchat-factor-method", method, &options, false);
                        if *method != before {
                            state.code.clear();
                        }
                        ui.add_space(6.0);
                    }
                    let hint = match method {
                        TwoFactor::Totp => tr("The six digits your authenticator app shows for VRChat now."),
                        TwoFactor::Email => tr("VRChat emailed you six digits: check your inbox and spam folder."),
                        TwoFactor::Recovery => tr("One of the recovery codes you saved when you turned two-factor authentication on."),
                    };
                    let mut enter = false;
                    let six = *method != TwoFactor::Recovery;
                    widgets::field(ui, method.label(), Some(hint), if six { 304.0 } else { 300.0 }, |ui| {
                        let response = if six {
                            code_cells(ui, &mut state.code)
                        } else {
                            widgets::text_field(ui, "vrchat-factor-code", &mut state.code, "xxxx-xxxx", 300.0, true)
                        };
                        enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    widgets::separator(ui);
                    ui.add_space(14.0);
                    let ready = if six { state.code.chars().count() == 6 } else { !state.code.trim().is_empty() };
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let verify =
                        Button::new(if busy { tr("Verifying…") } else { tr("Verify and sign in") }).icon(icons::SHIELD_CHECK).variant(Variant::Primary).enabled(ready);
                    let cancel = Button::new(tr("Cancel")).variant(Variant::Ghost);
                    let (vw, cw) = (verify.desired_width(ui), cancel.desired_width(ui));
                    if (vrchat_pressed(verify.show_at(ui, Rect::from_min_size(pos2(row.right() - vw, row.top()), vec2(vw, 40.0)))) || enter) && ready {
                        command = Some(Command::Verify { method: *method, code: Secret(std::mem::take(&mut state.code)) });
                    }
                    if vrchat_pressed(cancel.show_at(ui, Rect::from_min_size(pos2(row.right() - vw - 8.0 - cw, row.top()), vec2(cw, 40.0)))) {
                        command = Some(Command::Cancel);
                    }
                }
            });
        }
        if busy && !connecting {
            ui.add_space(12.0);
            super::sharing::working(ui, tr("Waiting for VRChat…"));
        }
        if let Some(error) = &status.error {
            ui.add_space(12.0);
            orbiters_ui::feedback::banner(ui, Id::unique(("vrchat-login-error", error)), widgets::Tone::Danger, error, false);
            let retry = match status.recovery {
                Recovery::Resume => Some((tr("Retry the saved session"), icons::ARROW_CLOCKWISE, Command::Resume)),
                Recovery::RemoveSavedSession => Some((tr("Remove the saved session"), icons::TRASH, Command::SignOut)),
                Recovery::None => None,
            };
            if let Some((label, glyph, retry)) = retry {
                ui.add_space(10.0);
                let b = Button::new(label).icon(glyph).variant(Variant::Secondary).size(Size::Sm).enabled(!busy);
                let bw = b.desired_width(ui);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                if vrchat_pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                    command = Some(retry);
                }
            }
        }
        ui.add_space(4.0);
    });
    if let Some(command) = command {
        state.pending = true;
        state.pending_revision = status.revision;
        state.saw_busy = false;
        state.code.clear();
        cx.ctl.vrchat_login(command);
        orbiters_ui::anim::request_frame(ui.ctx());
    }
}
/// Folder row: label, path, Change button. True when Change was clicked.
fn folder_field(ui: &mut Ui, label: &str, description: Option<&str>, value: &std::path::Path) -> bool {
    let shown = value.display().to_string();
    let mut clicked = false;
    let desc = description.unwrap_or(&shown);
    widgets::field(ui, label, Some(desc), if description.is_some() { 380.0 } else { 110.0 }, |ui| {
        clicked = Button::new(tr("Change")).icon(icons::FOLDER).size(Size::Sm).show(ui).clicked();
        if description.is_some() {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(260.0, 20.0), Sense::hover());
            widgets::text_truncated(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(&shown, 12.0).color(theme::FG_3), 260.0);
        }
    });
    clicked
}

/// The system's default microphone, as a choice among microphones (each system names its own).
pub(crate) fn default_mic() -> &'static str {
    tr(crate::words::DEFAULT_MIC)
}

/// A chip to choose a source (a Spout sender, a microphone, a sound device); `key` names the choice it makes, for the
/// registry of Settings ([`widgets::mark_control`]).
pub(super) fn source_chip(ui: &mut Ui, key: &str, label: &str, active: bool) -> egui::Response {
    let t = Txt::new(label, 12.0).color(if active { theme::FG } else { theme::FG_2 });
    let s = widgets::text_size(ui, &t);
    let (r, resp) = ui.allocate_exact_size(vec2(s.x + 24.0, 30.0), Sense::click());
    widgets::mark_control(key, "chip", r);
    let hover = anim::toggle(ui.ctx(), resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let press = anim::toggle(ui.ctx(), resp.id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let rr = Rect::from_center_size(r.center(), r.size() * (1.0 - 0.05 * press));
    let bg = if active { theme::INK_600 } else { theme::mix(theme::control(), theme::INK_600, 0.5 * hover) };
    ui.painter().rect_filled(rr, theme::radius(15.0), bg);
    widgets::text(ui.painter(), rr.center(), Align2::CENTER_CENTER, &t);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// How Orbiters appears around VRChat's menus: a switch for each of them, how the tiles follow the hand, and what stands
/// in the way now. (Where N4N0's own menu shows is with N4N0's menu, in [`super::settings_n4n0::menu_card`].)
fn orbit_settings(ui: &mut egui::Ui, snap: &Snapshot, orbit: &mut orbiters_core::settings::OrbitSettings, reset: &mut bool) -> bool {
    let mut apply = false;
    widgets::field(ui, tr("Around the Action Menu's wheel"), Some(tr("Orbiters around VRChat's own wheel, with it, whatever N4N0's menu does: opened with R in desktop mode, or by holding the menu button in VR.")), 48.0, |ui| {
        apply |= widgets::toggle(ui, "orbit-wheel", &mut orbit.wheel);
    });
    widgets::separator(ui);
    widgets::field(ui, tr("Around the camera"), Some(tr("3D pictures, Record and the QR code beside VRChat's camera. In a headset they appear where VRChat opens it and move with the hand that holds it; in desktop mode, at the left of its panel.")), 48.0, |ui| {
        apply |= widgets::toggle(ui, "orbit-camera", &mut orbit.camera_tiles);
    });
    if orbit.camera_tiles {
        widgets::separator(ui);
        widgets::field(ui, tr("Find the camera on screen"), Some(tr("Also looks for the camera's panel in what VRChat shows and moves the tiles onto it. Off by default: it can put them in the wrong place. It keeps nothing it sees.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "orbit-camera-pattern", &mut orbit.camera_pattern);
        });
    }
    if let Some(why) = snap.orbit.camera_note.as_ref().filter(|_| orbit.camera_tiles) {
        widgets::separator(ui);
        widgets::field(ui, tr("The camera's tiles, right now"), Some(tr(why)), 48.0, |_| {});
    }
    widgets::separator(ui);
    widgets::field(ui, tr("Turn around in a 3D capture"), Some(tr("In a headset, with a capture around you: VRChat's turning stick turns what you see, smoothly or in steps as VRChat is set to.")), 48.0, |ui| {
        apply |= widgets::toggle(ui, "orbit-immersive-turn", &mut orbit.immersive_turn);
    });
    widgets::separator(ui);
    widgets::field(ui, tr("Around the Quick Menu"), Some(tr("Orbiters beside VRChat's own Quick Menu, with it, whatever N4N0's menu does: opened with Esc in desktop mode, or by a short press of the menu button in VR.")), 48.0, |ui| {
        apply |= widgets::toggle(ui, "orbit-quick-menu", &mut orbit.quick_menu);
    });
    widgets::separator(ui);
    let sizes = [SegOption::new(0.8f32, tr("Smaller")), SegOption::new(1.0f32, tr("As measured")), SegOption::new(1.25f32, tr("Larger"))];
    widgets::field(ui, tr("Panels' size in VR"), Some(tr("If they sit too near or too far from VRChat's menu in your headset.")), 300.0, |ui| {
        apply |= widgets::segmented(ui, "orbit-vr-size", &mut orbit.vr_size, &sizes, true);
    });
    widgets::separator(ui);
    use orbiters_core::settings::VrMotion;
    let motions = [SegOption::new(VrMotion::Springy, tr("Springy")), SegOption::new(VrMotion::Matched, tr("Match VRChat"))];
    let said = match orbit.vr_motion {
        VrMotion::Springy => tr("The tiles around VRChat's menus each follow your hand on a little spring of their own."),
        VrMotion::Matched => tr("The tiles around VRChat's menus stay rigid, and trail your hand as VRChat's menu does."),
    };
    widgets::field(ui, tr("How they follow your hand in VR"), Some(said), 300.0, |ui| {
        apply |= widgets::segmented(ui, "orbit-vr-motion", &mut orbit.vr_motion, &motions, true);
    });
    widgets::separator(ui);
    let smoothing = |v: f32| if v == 0.0 { tr_ctx("smoothing", "None").to_string() } else { format!("{v:.0} ms") };
    widgets::field(ui, tr("Smoothing on the Quick Menu"), Some(tr("How softly what is rigid follows your hand: what opens in front of VRChat's menu, always, and the tiles when they match it. About 35 ms lines up with VRChat.")), 300.0, |ui| {
        apply |= widgets::slider(ui, "orbit-vr-smoothing-quick-menu", &mut orbit.vr_smoothing_quick_menu_ms, 0.0..=150.0, 1.0, smoothing);
    });
    widgets::field(ui, tr("Smoothing on the wheel"), Some(tr("The same around VRChat's Action Menu. About 20 ms lines up with VRChat.")), 300.0, |ui| {
        apply |= widgets::slider(ui, "orbit-vr-smoothing-wheel", &mut orbit.vr_smoothing_wheel_ms, 0.0..=150.0, 1.0, smoothing);
    });
    // Both back to what was measured to match VRChat's own menus (the defaults), at once, as the sliders write them.
    let (quick, wheel) = orbiters_core::settings::OrbitSettings::measured_smoothing();
    let back = tr!("Back to {quick} on the Quick Menu and {wheel} on the wheel: the values measured to match VRChat's own menus.", quick = smoothing(quick), wheel = smoothing(wheel));
    widgets::field(ui, tr("Reset the smoothing"), Some(&back), 300.0, |ui| {
        let done = orbit.smoothing_as_measured();
        let b = Button::new(tr("Reset"))
            .icon(icons::ARROW_COUNTER_CLOCKWISE)
            .variant(Variant::Secondary)
            .size(Size::Sm)
            .enabled(!done)
            .id_salt("orbit-vr-smoothing-reset")
            .hint(if done { tr("Both are already the values measured to match VRChat") } else { tr("Puts both smoothings back to the values measured to match VRChat") });
        if b.show(ui).clicked() {
            orbit.vr_smoothing_quick_menu_ms = quick;
            orbit.vr_smoothing_wheel_ms = wheel;
            apply = true;
            *reset = true;
        }
    });
    let now = if !orbiters_core::osc::VRCHAT_RUNS_HERE {
        Some(tr("VRChat runs on another computer here, so nothing can appear around its menus.").to_string())
    } else if let Some(note) = &snap.orbit.note {
        Some(tr(note).to_string())
    } else if snap.vrchat.running {
        None
    } else {
        Some(tr("They appear once VRChat runs.").to_string())
    };
    if let Some(now) = now {
        widgets::separator(ui);
        widgets::field(ui, tr("Right now"), Some(&now), 48.0, |_| {});
    }
    apply
}

/// How many notices Settings shows (the file keeps more).
const LOG_SHOWN: usize = 400;

/// Every notice the app showed (`orbiters_core::notice_log`), newest first: a toast is gone in seconds, and an error
/// nobody had the time to read is still here. They are rows of the page itself (it is the one place that scrolls), the
/// first ones at once and the others a press of Show more away.
fn notice_log(ui: &mut Ui, shown: &mut usize) {
    let logs = orbiters_core::paths::logs_dir();
    let file = orbiters_core::notice_log::file(&logs);
    // Read again only when the file changed (looked at once a second).
    let id = Id::unique("settings-notice-log");
    let changed = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let kept: Option<(Option<std::time::SystemTime>, std::sync::Arc<Vec<orbiters_core::notice_log::Entry>>)> = ui.ctx().data(|d| d.get_temp(id));
    let entries = match kept {
        Some((at, entries)) if at == changed => entries,
        _ => {
            let entries = std::sync::Arc::new(orbiters_core::notice_log::newest(&logs, LOG_SHOWN));
            ui.ctx().data_mut(|d| d.insert_temp(id, (changed, entries.clone())));
            entries
        }
    };
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    ui.add_space(8.0);
    if entries.is_empty() {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
        widgets::text(ui.painter(), pos2(row.left(), row.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Nothing yet."), 13.0).color(theme::FG_3));
        return;
    }
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let mut bu = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(egui::Align::Center)));
    bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
    if Button::new(tr("Show the file")).icon(icons::FOLDER_OPEN).variant(Variant::Ghost).size(Size::Sm).id_salt("notice-log-file").show(&mut bu).clicked() {
        let _ = super::editor::files::reveal(&file);
    }
    if Button::new(tr("Copy all")).icon(icons::COPY).variant(Variant::Ghost).size(Size::Sm).id_salt("notice-log-copy").show(&mut bu).clicked() {
        let text: Vec<String> = entries.iter().map(|e| format!("{}  {}  {}", super::local_time_unix(e.at.max(0) as u64), e.kind, e.text)).collect();
        ui.ctx().copy_text(text.join("\n"));
    }
    *shown = (*shown).max(LOG_FIRST);
    {
        for entry in entries.iter().take(*shown) {
            let colour = match entry.kind.as_str() {
                "error" => theme::DANGER,
                "warning" => theme::WARN,
                "success" => theme::OK,
                "detail" => theme::FG_4,
                _ => theme::FG_3,
            };
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            if !ui.is_rect_visible(row) {
                continue;
            }
            let p = ui.painter();
            p.circle_filled(pos2(row.left() + 5.0, row.center().y), 3.5, colour);
            let time = widgets::text(p, pos2(row.left() + 18.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(super::local_time_unix(entry.at.max(0) as u64), 12.0).color(theme::FG_3).mono());
            let one_line = entry.text.replace('\n', " ");
            let text_colour = if entry.kind == "detail" { theme::FG_3 } else { theme::FG_2 };
            widgets::text_truncated(p, pos2(time.right() + 12.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(&one_line, 12.5).color(text_colour), row.right() - time.right() - 20.0);
            ui.interact(row, id.with(("row", entry.at, one_line.len())), Sense::hover()).on_hover_text(&entry.text);
        }
    }
    if entries.len() > *shown {
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
        let more = tr!("Show more ({count} more)", count = entries.len() - *shown);
        if Button::new(&more).variant(Variant::Ghost).size(Size::Sm).id_salt("notice-log-more").show(&mut bu).clicked() {
            *shown += 100;
        }
        ui.add_space(8.0);
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
