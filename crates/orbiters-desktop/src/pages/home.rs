//! Home: N4N0's menu as a page of the window. Its features (fast walk, YouTube, calls, a 3D picture, music, the light,
//! the avatar's saved states, tracking, suggest, previous instances, the QR code scanner) are cards like the rest of the
//! app's, and each opens a page of its own drawn the
//! same way; what N4N0 says fills the grid's last row. What they show and what a press does are the menu's
//! (`orbiters_n4n0_core::menu`, run by [`crate::menu_host`] in step with the tray service like the menu in VR); only the
//! drawing is the window's (the States page is [`super::states`]). Cards and choices act on the pointer's press, buttons
//! on its release with the press shown at once, and the menu shows a change before the service answers.

use super::{block, row_columns, section, section_up_to, Cx};
use crate::ipc::N4n0Command;
use crate::menu_host::MenuHost;
use crate::overlay::menu_ui;
use crate::state::StateAction;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align, Align2, Color32, CursorIcon, Id, Key, Layout, Painter, Pos2, Rect, Sense, Stroke, TextureId, Ui, UiBuilder, Vec2};
use orbiters_contracts::n4n0::{LIGHT_COLORS, LIGHT_LEVELS};
use orbiters_core::settings::Settings;
use orbiters_i18n::{mark, tr, tr_ctx};
use orbiters_n4n0_core::menu::layout::{self, Content, Icon, Tint, HOME_TILES, MORE_CARDS_QR};
use orbiters_n4n0_core::menu::strings::{self, Strings, Text};
use orbiters_n4n0_core::menu::{Call, Effect, Media, MediaCommand, Menu, Page, PhotoLength, PhotoMode, PlayingVolume, Target, Volume, TIMERS};
use orbiters_n4n0_core::youtube::Video;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Size, Tone, Variant};
use std::time::Duration;

/// The page's width at most, as the Settings cards'.
const MAX_W: f32 = 980.0;
/// The narrowest a list's row gets beside another one (results, instances on a wide page), and the room between them.
const ROW_MIN_W: f32 = 560.0;
const ROW_GAP: f32 = 32.0;
/// Home's feature cards, the room between them, and their insets (the Settings cards').
const CARD_H: f32 = 128.0;
const GAP: f32 = 16.0;
const INSET: f32 = 24.0;
/// A result's thumbnail.
const THUMB: Vec2 = vec2(160.0, 90.0);
/// How long a result's button says its link was copied.
const COPIED_FOR: f64 = 2.0;
/// How often the music is looked at while Home or the music page shows (on a thread: [`super::Watch`]).
const MUSIC_EVERY: Duration = Duration::from_millis(500);

/// The music as a page shows it: what plays, the volume and the art's key, with where it is in its timeline only for
/// the music page (`positions`). Home's card names what plays and nothing more, so a song going on is no change there.
type Music = (Option<Media>, Option<Volume>, Option<String>);

fn music_shown(reading: &orbiters_n4n0::media::Reading, positions: bool) -> Music {
    let mut media = reading.media.clone();
    if let (Some(media), false) = (media.as_mut(), positions) {
        media.position = None;
    }
    (media, reading.volume, reading.art.as_ref().map(|art| art.key.clone()))
}

/// A text of N4N0's own menu in the language now (`orbiters-n4n0-core/menu-strings`): the words the menu in VR and on the avatar uses, so
/// the window and the menu say a timer, a flight, a colour of the light or a kind of tracker the same way.
fn menu_text(text: Text) -> &'static str {
    Strings::builtin(orbiters_i18n::language()).get(text)
}

/// The window's features, as All tools shows them: every tile of the avatar's Home, then previous instances and the QR
/// code scanner (More's, on the avatar's screen).
fn features() -> impl Iterator<Item = (Target, Icon, Text, Tint)> {
    HOME_TILES.into_iter().chain(MORE_CARDS_QR.into_iter().filter(|t| matches!(t.0, Target::Instances | Target::Qr))).filter(|t| available(t.0))
}

/// The features Home itself shows (REQUEST21 6: "remove all the tiles except: Call, Suggest, Tracking"), in the owner's
/// order. The others are one press away, under All tools, and each also where it belongs: fast walk in Settings'
/// Performance, the 3D picture in Photos & captures' capture bar, previous instances in Where.
const HOME_ONLY: [Target; 3] = [Target::Call, Target::Suggest, Target::Tracking];

fn home_tiles() -> impl Iterator<Item = (Target, Icon, Text, Tint)> {
    HOME_ONLY.into_iter().filter_map(|target| features().find(|t| t.0 == target))
}

/// Whether a feature can work here: fast walk moves SteamVR's play space, Tracking smooths SteamVR's devices through its
/// driver, and a 3D picture and the QR code scanner read VRChat's camera over Spout. Everything else goes over OSC or the web.
fn available(target: Target) -> bool {
    use orbiters_core::here::HERE;
    match target {
        Target::FastWalk => HERE.steam_vr,
        Target::Tracking => HERE.driver,
        Target::Photo | Target::Qr => HERE.camera_frames,
        _ => true,
    }
}

/// What a press asks of the menu, done once the page is drawn.
enum Act {
    Press(Target),
    /// The search field's new text.
    Query(String),
    /// Calls the friend at this place in the list.
    Call(usize),
    /// The speakers' volume, in percent.
    Volume(u8),
    /// What plays, to this second (its timeline dragged or clicked).
    Seek(u32),
    /// What the States page asks of the service (`Some`: another avatar's states than the worn one's).
    States(StateAction, Option<String>),
    /// The suggestion's new text.
    Suggestion(String),
    /// A kind of device's smoothing, in milliseconds (its slider).
    Smoothing(usize, u16),
    /// Back to Home itself.
    Home,
    /// Settings, VRChat account (Previous instances' Connect VRChat, REQUEST20 4).
    VrchatAccount,
    /// All tools opened, or closed back to Home (REQUEST21 6).
    Tools(bool),
}

#[derive(Default)]
pub struct State {
    /// Made with the window's context on the first visit (thumbnails become its textures), opened on Home.
    host: Option<MenuHost>,
    /// The page shown (YouTube's search and results are one), since when, and how many were opened: a feature's page
    /// enters like a page of its own.
    shown: Option<Page>,
    since: f64,
    opened: u64,
    /// The search field takes the keyboard when YouTube opens.
    focus: bool,
    /// The result whose link was copied last, and when.
    copied: Option<(String, f64)>,
    /// The States page's own state (what was asked and not yet seen, a name being typed).
    states: super::states::StatesPage,
    /// VRChat on the network, shown here until one is connected.
    network: super::network::State,
    /// All tools shows (every feature of N4N0's menu) instead of Home's three (REQUEST21 6); a feature opened from it
    /// comes back to it.
    all_tools: bool,
    /// What going back to Home asked of the menu's host from outside a frame (the sidebar's Home: the QR code scanner
    /// stops), done at the next one.
    pending: Vec<Effect>,
    /// The music, looked at on a thread while a page shows it (`true`: with where it is in its timeline): the page is
    /// drawn again only when what it shows of the music changed.
    music: super::Watch<bool, Music>,
    /// The VRChat launcher on Home itself: the friends list, Play, invites, profiles (REQUEST10).
    launcher: super::launcher::State,
    /// The groups the account runs, at Home's bottom, and a group's page in Home's place (REQUEST22).
    groups: super::groups::State,
}

impl State {
    /// The feature's page shown on Home, if one is (the usage statistics name it).
    pub fn shown(&self) -> Option<Page> {
        self.shown.filter(|p| *p != Page::Home)
    }

    /// Something on Home is in use (`crate::window_rest`): a profile's window open, or the music page, which plays,
    /// pauses and sets the volume of what plays.
    pub fn holds(&self) -> bool {
        self.launcher.holds() || self.shown() == Some(Page::Music)
    }

    /// A sheet or dialog of Home's is up over it (a profile, Previous instances, an edit of one's own card): it owns the
    /// keyboard and the pointer (N4N0's chat stands aside, verify-r21 major 7).
    pub fn modal_up(&self) -> bool {
        self.launcher.holds() || self.groups.holds()
    }

    /// Back to Home itself (the sidebar's Home pressed on a feature's page, or on All tools).
    pub fn back_home(&mut self) {
        self.groups.close();
        self.all_tools = false;
        if let Some(host) = self.host.as_mut() {
            self.pending.extend(go_home(host));
        }
    }

    /// Home itself, its Groups section in view (N4N0's assistant: "open my groups"; REQUEST22 1).
    pub fn open_groups(&mut self) {
        self.back_home();
        self.groups.reveal();
    }

    /// All tools shows, every feature of N4N0's menu (screenshots, tests).
    pub fn open_all_tools(&mut self) {
        self.all_tools = true;
    }

    /// The window leaves Home for another page (the sidebar): the QR code scanner its page holds stops, the friends its
    /// Share to … holds are forgotten, now (MENU-13: nothing reads the camera for a page nobody sees); shown again, its
    /// page takes them up again.
    pub fn left(&mut self, backend: &crate::backend::Ctl) {
        // A group's write not sent yet is over with its view (B-F).
        self.groups.end_flow();
        if let Some(host) = self.host.as_mut() {
            host.let_go(backend);
            self.pending.extend(host.menu.take_up());
        }
    }

    /// The menu Home runs, once shown (the screenshot review opens its pages).
    pub fn menu_host(&mut self) -> Option<&mut MenuHost> {
        self.host.as_mut()
    }

    /// The launcher on Home (the controller hands it what it knows; the screenshot review stages it).
    pub fn launcher_mut(&mut self) -> &mut super::launcher::State {
        &mut self.launcher
    }

    /// The Groups section and a group's page (the screenshot review stages them).
    pub fn groups_mut(&mut self) -> &mut super::groups::State {
        &mut self.groups
    }

    /// The States page (the screenshot review opens an avatar's states).
    pub fn states_mut(&mut self) -> &mut super::states::StatesPage {
        &mut self.states
    }
}

/// YouTube's search and its results are one page here.
fn shown_page(page: Page) -> Page {
    match page {
        Page::Results => Page::Search,
        page => page,
    }
}

/// Back to Home (results go back to the search, the search to Home), with what leaving a page asks for (the QR code
/// scanner stops).
fn go_home(host: &mut MenuHost) -> Vec<Effect> {
    let mut effects = Vec::new();
    for _ in 0..2 {
        if host.menu.page != Page::Home {
            effects.extend(host.press(Target::Back));
        }
    }
    effects
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    // A group's page (or the banner studio) takes Home's place (REQUEST22).
    if state.groups.page_open() {
        return super::groups::show_page(ui, cx, &mut state.groups);
    }
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let State { host, shown, since, opened, focus, copied, states, pending, network, all_tools, music: music_watch, launcher, groups } = state;
    launcher.wire(&ctx, cx);
    let host = host.get_or_insert_with(|| {
        // What it fetches (YouTube's answers, thumbnails) asks for a frame when it arrives.
        let mut host = MenuHost::waking(ctx.clone()).holding(orbiters_n4n0::MenuHolder::Window);
        host.open(snap);
        host
    });
    host.follow(snap);
    // The music card names what plays, and the music page shows it: read while one of them shows. The watch keeps
    // the readings coming between frames, and asks for one when what the page shows of them changed.
    if matches!(host.menu.page, Page::Home | Page::Music) {
        host.follow_media();
        let positions = host.menu.page == Page::Music;
        music_watch.keep(&ctx, &positions, MUSIC_EVERY, || {
            let media = crate::media::watcher().clone();
            move || {
                media.want();
                music_shown(&media.reading().1, positions)
            }
        });
    }
    let mut effects = host.poll();
    effects.append(pending);

    // A page opened enters as a page of its own: its blocks count from then (All tools too).
    let page = shown_page(host.menu.page);
    let now = anim::time(&ctx);
    let tools_shown = page == Page::Home && *all_tools;
    if *shown != Some(page) || ctx.data(|d| d.get_temp::<bool>(Id::unique("home-all-tools-was"))) != Some(tools_shown) {
        ctx.data_mut(|d| d.insert_temp(Id::unique("home-all-tools-was"), tools_shown));
        *shown = Some(page);
        *since = now;
        *opened += 1;
        *focus = page == Page::Search;
        // States opens on the avatars' cards.
        if page == Page::States {
            states.show_cards();
        }
    }
    let (visit, entered) = (cx.visit, cx.entered);
    cx.visit = (visit << 20) | *opened;
    cx.entered = entered.max(*since);

    let mut acts = Vec::new();
    // The 3D picture as Photos & captures' capture element allows it.
    let take_blocked = if !snap.settings.consent_acknowledged {
        Some(tr("Accept the capture notice in Photos & captures first"))
    } else if !snap.vrchat.running {
        Some(tr("Start VRChat first: its camera takes the picture"))
    } else {
        None
    };
    let idle = idle_line(&snap.settings);
    let menu = &host.menu;
    let picture = |key: &str| host.picture(key, false);
    let copied_id = copied.as_ref().filter(|(_, at)| now - at < COPIED_FOR).map(|(id, _)| id.as_str());
    let from = orbiters_ui::blur::next_shape(ui);
    // Home itself has the launcher on its right: the friends list (or its rail on a narrow window).
    let full = ui.max_rect();
    let side = (page == Page::Home && launcher.shown()).then(|| super::launcher::side(full.width()));
    let (content, right_margin) = match side {
        Some(s) => {
            let column = Rect::from_min_max(pos2(full.right() - super::launcher::GAP - s.width(), full.top() + cx.top + 8.0), pos2(full.right() - super::launcher::GAP, full.bottom() - super::launcher::GAP));
            (Rect::from_min_max(full.min, pos2(column.left(), full.bottom())), Some(column))
        }
        None => (full, None),
    };
    let mut left = ui.new_child(UiBuilder::new().max_rect(content).id_salt("home-content"));
    let margin_right = if right_margin.is_some() { 20 } else { 32 };
    // A friend's instance just chosen with Select (REQUEST23): Home's top, where Where shows it chosen.
    let mut scroll = egui::ScrollArea::vertical().id_salt(("home", page));
    if launcher.places.take_reveal() {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.auto_shrink([false, false]).scroll_bar_rect(super::below_bar(&left, cx)).show(&mut left, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: margin_right, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            match page {
                // The window never shows a dropped card itself (the avatar's screen does), nor the simple Home's More (its
                // Home has every feature).
                // World feedback is given from the world's window (Visited worlds) and the menus, not from this Home; the
                // screen mirror shows only on the avatar's screen.
                Page::Home | Page::Card | Page::More | Page::Feedback | Page::FeedbackLabels | Page::Comments | Page::Mirror => {
                    if *all_tools {
                        tools(ui, cx, menu, copied_id, &mut acts)
                    } else {
                        home(ui, cx, menu, &idle, copied_id, network, launcher, &mut acts);
                        super::groups::home_section(ui, cx, groups);
                    }
                }
                Page::Instances | Page::AddFriend => instances(ui, cx, menu, snap.vrchat_login.user.is_some(), &mut acts),
                Page::Search | Page::Results => youtube(ui, cx, menu, &picture, focus, copied_id, &mut acts),
                Page::Call => call(ui, cx, menu, &mut acts),
                Page::Photo => photo(ui, cx, menu, take_blocked, &mut acts),
                Page::Music => music(ui, cx, menu, &picture, copied_id, &mut acts),
                Page::Light => light(ui, cx, menu, &mut acts),
                Page::Tracking => tracking(ui, cx, menu, &mut acts),
                Page::Suggest | Page::Type => suggest(ui, cx, menu, &mut acts),
                Page::Qr => qr(ui, cx, menu, &mut acts),
                // The window shares captures in Photos & captures: its 3D picture page has no History.
                Page::History | Page::ShareTo | Page::Explicit => photo(ui, cx, menu, take_blocked, &mut acts),
                Page::States | Page::NewState => {
                    let mut out = Vec::new();
                    super::states::show(ui, cx, states, &mut out);
                    acts.extend(out.into_iter().map(|o| match o {
                        super::states::Out::Home => Act::Home,
                        super::states::Out::Action(action, avatar) => Act::States(action, avatar),
                    }));
                }
            }
        });
    });
    // Escape goes back to Home (to All tools from a feature opened there), unless the page took it for itself (a
    // rename it cancels): asked after the page.
    if page != Page::Home && ui.input(|i| i.key_pressed(Key::Escape)) {
        acts.insert(0, Act::Home);
    } else if page == Page::Home && *all_tools && !launcher.holds() && ui.input(|i| i.key_pressed(Key::Escape)) {
        acts.insert(0, Act::Tools(false));
    }
    // A feature's page says where its way back goes.
    ctx.data_mut(|d| d.insert_temp(Id::unique("home-back-to-tools"), *all_tools));
    // Down to where the content starts (its top margin).
    super::under_top(ui, cx, "home", from, ui.max_rect().top() + cx.top + 8.0);
    if let (Some(column), Some(side)) = (right_margin, side) {
        super::launcher::side_panel(ui, cx, launcher, column, side);
        super::launcher::over_page(ui, cx, launcher, full);
    }
    cx.visit = visit;
    cx.entered = entered;

    for act in acts {
        match act {
            // Call while calls are not open to this account: a plain word of why, and nothing opens.
            Act::Press(Target::Call) if host.menu.calls_soon => {
                cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Info, tr(CALLS_SOON_TOLD));
            }
            Act::Press(target) => {
                if let Target::Copy(i) = target {
                    if let Some(video) = host.menu.page_results().get(i as usize) {
                        *copied = Some((video.id.clone(), now));
                    }
                }
                // The YouTube video playing, as a result.
                if let (Target::CopyPlaying, Some(video)) = (target, host.menu.media.as_ref().and_then(|m| m.video.clone())) {
                    *copied = Some((video, now));
                }
                effects.extend(host.press(target));
            }
            Act::Query(text) => {
                // Typing after a search: back to the suggestions.
                if host.menu.page == Page::Results {
                    effects.extend(host.press(Target::Back));
                }
                effects.extend(host.menu.set_query(&text));
            }
            Act::Call(i) => effects.extend(host.menu.call_contact(i)),
            // The playing app's own volume (the computer's where apps have none), shown at once.
            Act::Volume(percent) => effects.extend(host.menu.set_playing_volume(percent)),
            // Shown where it goes at once; the computer follows.
            Act::Seek(at) => {
                if let Some(media) = host.menu.media.as_mut() {
                    media.position = Some(at);
                }
                effects.push(Effect::Media(MediaCommand::Seek(at)));
            }
            Act::States(action, avatar) => cx.ctl.n4n0(N4n0Command::States { action, avatar }),
            Act::Suggestion(text) => host.menu.suggestion = text,
            Act::Smoothing(kind, ms) => effects.extend(host.menu.set_smoothing_ms(kind, ms)),
            Act::Home => effects.extend(go_home(host)),
            Act::Tools(open) => *all_tools = open,
            Act::VrchatAccount => cx.go = Some(super::Go::VrchatAccount),
        }
    }
    // It stays open here (nothing closes it): a close goes back to Home. A dropped card is on the avatar's screen: the
    // results stay here.
    if host.effects(effects, snap, cx.ctl) {
        host.open(snap);
    }
    host.dropped = false;

    // What arrives on its own asks for its frame itself: YouTube's answers and thumbnails (the menu's host), the music
    // (its watch), what the service says (its news). Only what the clock alone brings is asked for here: what the
    // host waits for (suggestions once typing paused, a thumbnail tried again), a call's clock, a copied link's note
    // ending. The page used to draw itself two to ten times a second to look.
    if let Some(wait) = host.next_wake(std::time::Instant::now()) {
        ctx.request_repaint_after(wait.max(Duration::from_millis(1)));
    }
    let menu = &host.menu;
    if matches!(menu.call, Some(Call::Connected { .. })) {
        // Its seconds count on Home's card and on its page.
        ctx.request_repaint_after(if matches!(menu.page, Page::Home | Page::Music) { Duration::from_millis(500) } else { Duration::from_secs(1) });
    }
    if let Some((_, at)) = copied {
        let left = COPIED_FOR - (now - *at);
        if left > 0.0 {
            ctx.request_repaint_after(Duration::from_secs_f64(left));
        }
    }
}

/// What Home's heading says while N4N0 does not listen: how to call it from here.
fn idle_line(settings: &Settings) -> String {
    // Enzo: saying its name should not be the first thing offered: "almost nobody will want to do that".
    let _ = settings;
    // Talking to N4N0 is the bar at the top of the window, on every page (REQUEST21 3).
    tr("Pick a feature, or talk to N4N0 at the top of the window").into()
}

#[allow(clippy::too_many_arguments)]
fn home(ui: &mut Ui, cx: &mut Cx, menu: &Menu, idle: &str, copied: Option<&str>, network: &mut super::network::State, launcher: &mut super::launcher::State, acts: &mut Vec<Act>) {
    block(ui, cx, 0, 72.0, |ui, rect| {
        let p = ui.painter();
        widgets::text(p, pos2(rect.left(), rect.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps(tr("N4N0 menu")));
        widgets::text(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(tr("Home"), 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
        let y = rect.top() + 62.0;
        if menu.listening {
            // N4N0 listens: its dot breathes.
            widgets::status_dot(p, pos2(rect.left() + 5.0, y), Tone::Ok, true);
            let line = Txt::new(layout::text(menu, Content::Status), 13.0).color(theme::accent());
            widgets::text_truncated(p, pos2(rect.left() + 18.0, y), Align2::LEFT_CENTER, &line, rect.width() * 0.6);
        } else {
            widgets::text_truncated(p, pos2(rect.left(), y), Align2::LEFT_CENTER, &Txt::new(idle, 13.0).color(theme::fg_3()), rect.width() * 0.6);
        }
    });

    // Play in VR, Play in Desktop (always, with the VRChat profile they start on), an invite received with both in it;
    // then the account's own profile, changed where it shows (REQUEST15 7).
    // Where VRChat starts into (REQUEST20 2), above the Play buttons it goes with.
    super::launcher::destinations::row(ui, cx, launcher, 1);
    super::launcher::hero(ui, cx, launcher, 1);
    super::launcher::mine_card(ui, cx, launcher, 3);

    // Until a VRChat on the network is connected, the first thing to do: most of what follows goes through it.
    if !orbiters_core::osc::VRCHAT_RUNS_HERE && !matches!(cx.snap.lan.link, crate::state::LanLink::Connected | crate::state::LanLink::Off) {
        super::network::card_saving(ui, cx, 1, network);
    }

    let w = ui.available_width();
    // With the launcher beside them (Play, the account's profile, friends), the features take a compact row: they
    // matter more in game and in the overlay than in this window (REQUEST15 7c). Alone on Home, their cards. Home keeps
    // three of them (REQUEST21 6), side by side, and All tools, at the caption's end, has every one.
    let compact = launcher.shown();
    {
        let p = ui.painter().clone();
        let (cap, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        widgets::text(&p, pos2(cap.left() + 2.0, cap.center().y), Align2::LEFT_CENTER, &Txt::caps(tr("N4N0's features")));
        let more = Button::new(tr("All tools")).icon(icons::SQUARES_FOUR).variant(Variant::Ghost).size(Size::Sm).id_salt("home-all-tools").hint(tr("Every tool of N4N0's menu: fast walk, YouTube, the 3D picture, music, the light, saved states, previous instances, the QR code scanner"));
        let mw = more.desired_width(ui);
        if more.show_at(ui, Rect::from_min_size(pos2(cap.right() - mw, cap.top() - 1.0), vec2(mw, 32.0))).clicked() {
            acts.push(Act::Tools(true));
        }
        ui.add_space(-10.0);
    }
    let gap = if compact { COMPACT_GAP } else { GAP };
    let tiles: Vec<_> = home_tiles().collect();
    let count = tiles.len();
    // One row of them, as wide as the page; under each other where three would not keep their names.
    let narrow_min = if compact { COMPACT_MIN } else { TILE_MIN - 30.0 };
    let cols = if (w - gap * (count as f32 - 1.0)) / count as f32 >= narrow_min { count } else { 1 };
    let card_w = (w - gap * (cols - 1) as f32) / cols as f32;
    let card_h = if compact { COMPACT_H } else { tile_height(card_w.min(TILE_MAX)) };
    let rows = count.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + gap) - gap), Sense::hover());
    let cell = |i: usize| area.min + vec2((i % cols) as f32 * (card_w + gap), (i / cols) as f32 * (card_h + gap));
    for (i, tile) in tiles.into_iter().enumerate() {
        let rect = Rect::from_min_size(cell(i), vec2(card_w, card_h));
        if compact {
            feature_compact(ui, cx, i, tile, rect, menu, copied, acts);
        } else {
            feature(ui, cx, i, tile, rect, menu, copied, acts);
        }
    }
    // What N4N0 says, under them, the row's whole width.
    let said_h = if compact { COMPACT_H } else { CARD_H - 24.0 };
    let (at, _) = ui.allocate_exact_size(vec2(w, said_h), Sense::hover());
    said(ui, cx, 1 + count, at, menu);
}

/// All tools (REQUEST21 6): every feature of N4N0's menu as Home had them, the window's whole grid of cards, with what
/// N4N0 says in the rest of the last row. Back (or Escape) goes to Home.
fn tools(ui: &mut Ui, cx: &mut Cx, menu: &Menu, copied: Option<&str>, acts: &mut Vec<Act>) {
    // Home's own heading, its caption the way back (the grid stays where Home's was).
    block(ui, cx, 0, 72.0, |ui, rect| {
        let id = Id::unique("all-tools-back");
        let caption = Txt::caps(tr("Home"));
        let size = widgets::text_size(ui, &caption);
        let link = Rect::from_min_size(pos2(rect.left() - 6.0, rect.top() - 4.0), vec2(size.x + 34.0, 22.0));
        let resp = ui.interact(link, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Back to Home"));
        widgets::note_control(ui.ctx(), tr("Back to Home"), link, true);
        let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let p = ui.painter();
        p.rect_filled(link, theme::radius(11.0), theme::white(0.06 * hover));
        let ink = theme::mix(theme::fg_3(), theme::fg(), hover);
        widgets::icon(p, pos2(link.left() + 14.0, link.center().y), icons::ARROW_LEFT, 12.0, ink);
        widgets::text(p, pos2(link.left() + 26.0, rect.top() + 7.0), Align2::LEFT_CENTER, &caption.color(ink));
        widgets::text(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(tr("All tools"), 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
        let sub = Txt::new(tr("Every tool of N4N0's menu, as on your avatar's screen and in VR"), 13.0).color(theme::fg_3());
        widgets::text_truncated(p, pos2(rect.left(), rect.top() + 62.0), Align2::LEFT_CENTER, &sub, rect.width() * 0.8);
        if widgets::activated(&resp) {
            acts.push(Act::Tools(false));
        }
    });
    let w = ui.available_width();
    let cols = home_columns(w);
    let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
    let card_h = tile_height(card_w);
    // The features, then what N4N0 says in the rest of the last row (one place at least): the grid stays whole.
    let count = features().count();
    let cells = count + 1;
    let rows = cells.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + GAP) - GAP), Sense::hover());
    let cell = |i: usize| area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (card_h + GAP));
    for (i, tile) in features().enumerate() {
        feature(ui, cx, i, tile, Rect::from_min_size(cell(i), vec2(card_w, card_h)), menu, copied, acts);
    }
    let span = cols - count % cols;
    let at = cell(count);
    said(ui, cx, 1 + count, Rect::from_min_size(at, vec2(span as f32 * (card_w + GAP) - GAP, card_h)), menu);
}

/// Home's compact features: their height, the room between them, the narrowest before a row takes one fewer.
const COMPACT_H: f32 = 60.0;
const COMPACT_GAP: f32 = 10.0;
const COMPACT_MIN: f32 = 172.0;


/// The narrowest a Home tile gets before the row takes one fewer, the widest before it takes one more past six, and
/// the most tiles on a row (two rows of six hold every feature and what N4N0 says).
const TILE_MIN: f32 = 210.0;
const TILE_MAX: f32 = 440.0;
const MAX_COLUMNS: usize = 8;

/// Home's columns at `width`: as many tiles of [`TILE_MIN`] as fit, two to six (four on the default window); past six,
/// enough to keep tiles under [`TILE_MAX`].
fn home_columns(width: f32) -> usize {
    let fit = (((width + GAP) / (TILE_MIN + GAP)).floor() as usize).clamp(2, 6);
    let wide = ((width + GAP) / (TILE_MAX + GAP)).ceil() as usize;
    fit.max(wide).min(MAX_COLUMNS)
}

/// A tile's height at `width`: wider tiles on very wide windows grow a little taller, so they keep a card's shape.
fn tile_height(width: f32) -> f32 {
    CARD_H + ((width - 260.0) * 0.2).clamp(0.0, 40.0)
}

/// Call's line under its name while calls are not open to this account.
const CALLS_SOON_NOTE: &str = mark("Not open to you yet");
/// What a press on Call says while calls are not open to this account.
const CALLS_SOON_TOLD: &str = mark("Calls are coming soon to Orbiters - N4N0. They aren't open for your account yet.");

/// "Coming soon" in a greyed-out tile's top right corner, at full strength over the dimmed tile: the feature is on its way,
/// not broken. Its edge lights while pointed at.
fn soon_pill(ui: &Ui, r: Rect, alpha: f32, hover: f32) {
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let txt = Txt::new(tr(orbiters_n4n0_core::menu::CALLS_SOON), 11.5).weight(weight::SEMIBOLD).color(theme::fg_2());
    let size = p.layout_job(txt.job(f32::INFINITY)).size();
    let pill = Rect::from_min_size(pos2(r.right() - INSET - size.x - 18.0, r.top() + 24.0), vec2(size.x + 18.0, size.y + 8.0));
    p.rect_filled(pill, theme::radius(pill.height() / 2.0), theme::ink_800());
    if hover > 0.01 {
        p.rect_stroke(pill, theme::radius(pill.height() / 2.0), Stroke::new(1.0, theme::alpha(theme::accent(), hover)), egui::StrokeKind::Inside);
    }
    widgets::text(&p, pill.center(), Align2::CENTER_CENTER, &txt);
}

/// One of Home's features: its icon chip, its name and what it says now, with a switch for what turns on and off (fast
/// walk, the light), the Copy link of the YouTube video playing (music), or an arrow for what opens a page.
fn feature(ui: &mut Ui, cx: &Cx, i: usize, tile: (Target, Icon, Text, Tint), rect: Rect, menu: &Menu, copied: Option<&str>, acts: &mut Vec<Act>) {
    let (target, icon, title, tint) = tile;
    let title = menu_text(title);
    let state = layout::target_state(menu, target);
    let (rect, alpha) = super::entering(ui, cx, 1 + i, rect);
    let ctx = ui.ctx().clone();
    let id = Id::unique(("orbiters-home-feature", i));
    // "Coming soon" (Call without N4N0 calls) stays greyed but takes a click, which says why.
    let enabled = state.pointable();
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    widgets::note_control(&ctx, title, rect, enabled);
    if enabled && widgets::pressed(&resp) {
        acts.push(Act::Press(target));
    }
    let hover = anim::toggle(&ctx, id.with("hover"), enabled && resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.025 * press));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha * if state.unavailable { 0.5 } else { 1.0 });
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::ink_800(), hover));

    // The chip has the feature's colour: grey while what it switches is off, the light's own colour while it is on.
    let switched = matches!(target, Target::FastWalk | Target::Light);
    let color = match target {
        Target::Light => menu.light.filter(|l| l.on).map(|l| menu_ui::tint(Tint::Light(l.color))),
        _ if switched && !state.on => None,
        _ => Some(menu_ui::tint(tint)),
    };
    let (bg, fg) = match color {
        Some(c) => (theme::alpha(c, 0.12), c),
        None => (theme::white(0.05), theme::fg_3()),
    };
    let bg = anim::color(&ctx, id.with("chip-bg"), bg, Spring::SOFT);
    let fg = anim::color(&ctx, id.with("chip-fg"), fg, Spring::SOFT);
    let chip = Rect::from_min_size(r.min + vec2(INSET, 22.0), vec2(36.0, 36.0));
    p.rect_filled(chip, theme::radius(11.0), bg);
    widgets::icon(&p, chip.center(), menu_ui::glyph(icon), 17.0, fg);

    let x = r.left() + INSET;
    widgets::text(&p, pos2(x, r.bottom() - 46.0), Align2::LEFT_CENTER, &Txt::new(title, 15.5).weight(weight::SEMIBOLD));
    let active = state.on
        || match target {
            Target::Call => menu.call.is_some(),
            Target::Photo => menu.countdown.is_some(),
            Target::Music => menu.media.as_ref().is_some_and(|m| m.playing && !m.title.is_empty()),
            Target::States => menu.states.iter().any(|s| s.default),
            Target::Qr => menu.qr.scanning(),
            _ => false,
        };
    // Not open yet: the pill says "Coming soon", the note why.
    let note = if state.soon { tr(CALLS_SOON_NOTE).to_string() } else { layout::text(menu, Content::TileNote(layout::note_of(target))) };
    let note = Txt::new(note, 12.5).color(if active { theme::accent() } else { theme::fg_3() });
    widgets::text_truncated(&p, pos2(x, r.bottom() - 24.0), Align2::LEFT_CENTER, &note, r.width() - 2.0 * INSET);
    if state.soon {
        soon_pill(ui, r, alpha, hover);
    }

    // The YouTube video playing: its link copied from the card (pressed apart from it), the page not opened.
    let playing = match target {
        Target::Music => menu.media.as_ref().and_then(|m| m.video.as_deref()),
        _ => None,
    };
    if let Some(video) = playing {
        let (glyph, label) = if copied == Some(video) { (icons::CHECK, tr("Link copied")) } else { (icons::COPY, tr("Copy the video's link")) };
        let br = Rect::from_center_size(pos2(r.right() - INSET - 12.0, chip.center().y), Vec2::splat(36.0));
        let mut bu = ui.new_child(UiBuilder::new().max_rect(br).id_salt(("orbiters-home-copy-playing", i)));
        bu.multiply_opacity(alpha);
        if IconButton::new(glyph, label).id_salt("copy").show_at(&mut bu, br).clicked() {
            acts.push(Act::Press(Target::CopyPlaying));
        }
        return;
    }
    // On the right: a switch over the card (pressed apart from it), or the way in.
    let switch = match target {
        Target::FastWalk => Some(Target::FastWalk),
        Target::Light if menu.light.is_some() => Some(Target::LightSwitch),
        _ => None,
    };
    match switch {
        Some(switch) => {
            let sr = Rect::from_min_size(pos2(r.right() - INSET - 48.0, chip.center().y - 14.0), vec2(48.0, 28.0));
            let mut su = ui.new_child(UiBuilder::new().max_rect(sr).id_salt(("orbiters-home-switch", i)));
            su.multiply_opacity(alpha);
            if !enabled {
                su.disable();
            }
            let mut on = state.on;
            if widgets::toggle(&mut su, "switch", &mut on) {
                acts.push(Act::Press(switch));
            }
        }
        // ("Coming soon" stands there instead: the way in would show under it.)
        None if state.soon => {}
        None => widgets::icon(&p, pos2(r.right() - INSET - 8.0, chip.center().y), icons::CARET_RIGHT, 16.0, theme::mix(theme::fg_4(), theme::fg_2(), hover)),
    }
}

/// One of Home's features in the compact grid: its icon chip, its name over what it says now, and on the right its
/// switch, its Copy link or the way in, as [`feature`] has them.
#[allow(clippy::too_many_arguments)]
fn feature_compact(ui: &mut Ui, cx: &Cx, i: usize, tile: (Target, Icon, Text, Tint), rect: Rect, menu: &Menu, copied: Option<&str>, acts: &mut Vec<Act>) {
    let (target, icon, title, tint) = tile;
    let title = menu_text(title);
    let state = layout::target_state(menu, target);
    let (rect, alpha) = super::entering(ui, cx, 1 + i, rect);
    let ctx = ui.ctx().clone();
    let id = Id::unique(("orbiters-home-feature", i));
    // "Coming soon" (Call without N4N0 calls) stays greyed but takes a click, which says why.
    let enabled = state.pointable();
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    widgets::note_control(&ctx, title, rect, enabled);
    if enabled && widgets::pressed(&resp) {
        acts.push(Act::Press(target));
    }
    let hover = anim::toggle(&ctx, id.with("hover"), enabled && resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.03 * press));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha * if state.unavailable { 0.5 } else { 1.0 });
    widgets::frame(&p, r, 14.0, theme::mix(theme::surface(), theme::ink_800(), hover));
    let switched = matches!(target, Target::FastWalk | Target::Light);
    let color = match target {
        Target::Light => menu.light.filter(|l| l.on).map(|l| menu_ui::tint(Tint::Light(l.color))),
        _ if switched && !state.on => None,
        _ => Some(menu_ui::tint(tint)),
    };
    let (bg, fg) = match color {
        Some(c) => (theme::alpha(c, 0.12), c),
        None => (theme::white(0.05), theme::fg_3()),
    };
    let bg = anim::color(&ctx, id.with("chip-bg"), bg, Spring::SOFT);
    let fg = anim::color(&ctx, id.with("chip-fg"), fg, Spring::SOFT);
    let chip = Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 16.0), vec2(32.0, 32.0));
    p.rect_filled(chip, theme::radius(10.0), bg);
    widgets::icon(&p, chip.center(), menu_ui::glyph(icon), 15.0, fg);
    let active = state.on
        || match target {
            Target::Call => menu.call.is_some(),
            Target::Photo => menu.countdown.is_some(),
            Target::Music => menu.media.as_ref().is_some_and(|m| m.playing && !m.title.is_empty()),
            Target::States => menu.states.iter().any(|s| s.default),
            Target::Qr => menu.qr.scanning(),
            _ => false,
        };
    // On the right: a switch, the playing video's Copy link, or the way in.
    let switch = match target {
        Target::FastWalk => Some(Target::FastWalk),
        Target::Light if menu.light.is_some() => Some(Target::LightSwitch),
        _ => None,
    };
    let playing = match target {
        Target::Music => menu.media.as_ref().and_then(|m| m.video.as_deref()),
        _ => None,
    };
    // Not open yet: a "Coming soon" pill where the way in would be.
    let soon = state.soon.then(|| Txt::new(tr(orbiters_n4n0_core::menu::CALLS_SOON), 11.0).weight(weight::SEMIBOLD).color(theme::fg_2()));
    let soon_w = soon.as_ref().map_or(0.0, |t| p.layout_job(t.job(f32::INFINITY)).size().x + 16.0);
    let end = if switch.is_some() { 56.0 } else if playing.is_some() { 40.0 } else if soon.is_some() { soon_w + 18.0 } else { 24.0 };
    let x = chip.right() + 10.0;
    let room = r.right() - end - x;
    // Not open yet: its name alone, beside the pill that says so (a second line would be cut there).
    let title_y = if state.soon { r.center().y } else { r.center().y - 9.0 };
    widgets::text_truncated(&p, pos2(x, title_y), Align2::LEFT_CENTER, &Txt::new(title, 13.5).weight(weight::SEMIBOLD), room);
    if !state.soon {
        let note = Txt::new(layout::text(menu, Content::TileNote(layout::note_of(target))), 11.5).color(if active { theme::accent() } else { theme::fg_3() });
        widgets::text_truncated(&p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &note, room);
    }
    resp.on_hover_text(title);
    if let Some(txt) = soon {
        let size = p.layout_job(txt.job(f32::INFINITY)).size();
        let pill = Rect::from_min_size(pos2(r.right() - 12.0 - size.x - 16.0, r.center().y - size.y / 2.0 - 4.0), vec2(size.x + 16.0, size.y + 8.0));
        let mut pp = ui.painter().clone();
        pp.multiply_opacity(alpha);
        pp.rect_filled(pill, theme::radius(pill.height() / 2.0), theme::ink_800());
        if hover > 0.01 {
            pp.rect_stroke(pill, theme::radius(pill.height() / 2.0), Stroke::new(1.0, theme::alpha(theme::accent(), hover)), egui::StrokeKind::Inside);
        }
        widgets::text(&pp, pill.center(), Align2::CENTER_CENTER, &txt);
        return;
    }
    if let Some(video) = playing {
        let (glyph, label) = if copied == Some(video) { (icons::CHECK, tr("Link copied")) } else { (icons::COPY, tr("Copy the video's link")) };
        let br = Rect::from_center_size(pos2(r.right() - 24.0, r.center().y), Vec2::splat(32.0));
        let mut bu = ui.new_child(UiBuilder::new().max_rect(br).id_salt(("orbiters-home-copy-playing", i)));
        bu.multiply_opacity(alpha);
        if IconButton::new(glyph, label).size(32.0, 15.0).id_salt("copy").show_at(&mut bu, br).clicked() {
            acts.push(Act::Press(Target::CopyPlaying));
        }
        return;
    }
    match switch {
        Some(switch) => {
            let sr = Rect::from_min_size(pos2(r.right() - 12.0 - 48.0, r.center().y - 14.0), vec2(48.0, 28.0));
            let mut su = ui.new_child(UiBuilder::new().max_rect(sr).id_salt(("orbiters-home-switch", i)));
            su.multiply_opacity(alpha);
            if !enabled {
                su.disable();
            }
            let mut on = state.on;
            if widgets::toggle(&mut su, "switch", &mut on) {
                acts.push(Act::Press(switch));
            }
        }
        None => widgets::icon(&p, pos2(r.right() - 16.0, r.center().y), icons::CARET_RIGHT, 14.0, theme::mix(theme::fg_4(), theme::fg_2(), hover)),
    }
}

/// What N4N0 said last (or heard: the owner's own words), in the last row of the features: N4N0 beside its caption, the
/// words under them.
fn said(ui: &mut Ui, cx: &Cx, index: usize, rect: Rect, menu: &Menu) {
    let (r, alpha) = super::entering(ui, cx, index, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    if r.height() <= COMPACT_H + 0.5 {
        // A compact grid's: N4N0, its caption and its words on one line.
        let chip = Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 16.0), vec2(32.0, 32.0));
        p.rect_filled(chip, theme::radius(10.0), theme::alpha(theme::accent(), 0.12));
        n4n0_face(&p, chip.center(), 9.0);
        let caption = match (&menu.heard, menu.heard_private) {
            (None, _) => "N4N0",
            (Some(_), true) => tr("You said"),
            (Some(_), false) => tr("N4N0 says"),
        };
        widgets::text(&p, pos2(chip.right() + 12.0, r.center().y - 9.0), Align2::LEFT_CENTER, &Txt::caps(caption));
        let (text, color) = match menu.heard.as_deref() {
            Some(text) => (format!("“{text}”"), theme::fg_2()),
            None => (tr("Nothing said yet: N4N0's answers show here.").to_string(), theme::fg_3()),
        };
        widgets::text_truncated(&p, pos2(chip.right() + 12.0, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(text, 12.5).color(color), r.right() - chip.right() - 24.0);
        return;
    }
    let chip = Rect::from_min_size(r.min + vec2(INSET, 22.0), vec2(36.0, 36.0));
    p.rect_filled(chip, theme::radius(11.0), theme::alpha(theme::accent(), 0.12));
    n4n0_face(&p, chip.center(), 10.0);
    let caption = match (&menu.heard, menu.heard_private) {
        (None, _) => "N4N0",
        (Some(_), true) => tr("You said"),
        (Some(_), false) => tr("N4N0 says"),
    };
    widgets::text(&p, pos2(chip.right() + 12.0, chip.center().y), Align2::LEFT_CENTER, &Txt::caps(caption));
    let (text, color) = match menu.heard.as_deref() {
        Some(text) => (format!("“{text}”"), theme::fg_2()),
        None => (tr("Nothing said yet: N4N0's answers show here.").to_string(), theme::fg_3()),
    };
    let mut job = Txt::new(text, 13.0).color(color).line_height(18.0).job(r.width() - 2.0 * INSET);
    job.wrap.max_rows = 2;
    let lines = p.layout_job(job);
    p.galley(pos2(r.left() + INSET, chip.bottom() + 10.0), lines, color);
}

/// N4N0 itself, `radius` large: its green dot with its eyes, as the menu's back shows it.
fn n4n0_face(p: &Painter, center: Pos2, radius: f32) {
    let s = radius / 26.0;
    p.circle_filled(center, radius, theme::accent());
    for side in [-1.0, 1.0] {
        let eye = Rect::from_center_size(center + vec2(side * 9.0 * s, -2.0 * s), vec2(7.0 * s, 13.0 * s));
        p.rect_filled(eye, theme::radius(3.5 * s), menu_ui::CARD);
    }
}

/// The top of a feature's page: back to Home (or to All tools, when it was opened there), over its heading.
fn top(ui: &mut Ui, cx: &Cx, title: &str, sub: &str, acts: &mut Vec<Act>) {
    let to_tools = ui.ctx().data(|d| d.get_temp::<bool>(Id::unique("home-back-to-tools"))).unwrap_or(false);
    block(ui, cx, 0, 116.0, |ui, rect| {
        let back = Button::new(if to_tools { tr("All tools") } else { tr("Home") }).icon(icons::ARROW_LEFT).variant(Variant::Ghost).size(Size::Sm);
        let w = back.desired_width(ui);
        if back.show_at(ui, Rect::from_min_size(rect.min, vec2(w, 32.0))).clicked() {
            acts.push(Act::Home);
        }
        super::heading(ui, Rect::from_min_max(pos2(rect.left(), rect.top() + 44.0), rect.max), tr("N4N0 menu"), title, Some(sub));
    });
}

/// A card that only explains (nothing to show, nothing to call yet): its heading and a few lines under it.
fn note_card(ui: &mut Ui, cx: &Cx, index: usize, glyph: &str, title: &str, text: &str) {
    section(ui, cx, index, glyph, title, None, |ui| {
        let w = ui.available_width() - 48.0;
        let lines = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::fg_3()).line_height(19.0).job(w));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines.size().y + 8.0), Sense::hover());
        ui.painter().galley(pos2(r.left() + 48.0, r.top()), lines, theme::fg_3());
    });
}

/// A row of a card that acts on the pointer's press (a suggestion), a little lighter under the pointer; returns its rect,
/// how much it is hovered, and whether it was pressed.
fn press_row(ui: &mut Ui, id: Id, height: f32) -> (Rect, f32, bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = widgets::pressed(&resp);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let down = anim::toggle(ui.ctx(), id.with("down"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    if hover + down > 0.0 {
        ui.painter().rect_filled(rect.expand2(vec2(12.0, 0.0)), theme::radius(12.0), theme::white(0.04 * hover + 0.05 * down));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand);
    (rect, hover, pressed)
}

/// A round icon button, or its icon greyed out while it does nothing; true when clicked.
fn icon_button(ui: &mut Ui, rect: Rect, glyph: &str, label: &str, enabled: bool) -> bool {
    if enabled {
        return IconButton::new(glyph, label).size(rect.width(), 18.0).show_at(ui, rect).clicked();
    }
    widgets::icon(ui.painter(), rect.center(), glyph, 18.0, theme::alpha(theme::fg_4(), 0.6));
    false
}

/// A picture with rounded corners, or its place while it loads.
fn thumbnail(p: &Painter, r: Rect, texture: Option<TextureId>, radius: f32) {
    match texture {
        Some(id) => {
            p.add(RectShape::filled(r, theme::radius(radius), Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            p.rect_filled(r, theme::radius(radius), theme::surface_soft());
        }
    }
}

fn youtube(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, focus: &mut bool, copied: Option<&str>, acts: &mut Vec<Act>) {
    top(ui, cx, "YouTube", tr("Find a video, then copy its link for the world's video player"), acts);
    let w = ui.available_width().min(MAX_W);
    block(ui, cx, 1, 40.0, |ui, rect| {
        let button_w = 104.0;
        let field = Rect::from_min_size(rect.min, vec2(w - button_w - 12.0, 40.0));
        let mut fu = ui.new_child(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(Align::Center)));
        let mut text = menu.query.clone();
        let resp = widgets::search_field(&mut fu, "orbiters-home-search", &mut text, tr("Search YouTube"), field.width());
        if std::mem::take(focus) {
            resp.request_focus();
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let search = Button::new(tr("Search")).variant(Variant::Primary).enabled(!text.trim().is_empty()).width(button_w);
        let clicked = search.show_at(ui, Rect::from_min_size(pos2(field.right() + 12.0, rect.top()), vec2(button_w, 40.0))).clicked();
        if text != menu.query {
            acts.push(Act::Query(text.clone()));
        }
        if (enter || clicked) && !text.trim().is_empty() {
            acts.push(Act::Press(Target::Search));
        }
    });
    // The card dropped from here, out in the world: taken back from the top of the page.
    card_out(ui, cx, 2, acts);
    match menu.page {
        Page::Results => results(ui, cx, menu, picture, copied, acts),
        _ => suggestions(ui, cx, menu, acts),
    }
}

/// The link card out in the world (dropped from the results or the music): its title and link, the time it has left,
/// and Take back.
fn card_out(ui: &mut Ui, cx: &Cx, index: usize, acts: &mut Vec<Act>) {
    let Some(card) = cx.snap.n4n0.card.as_ref() else { return };
    let left = match card.left {
        0..60 => tr("less than a minute left").to_string(),
        s => tr!("{minutes} min left", minutes = s.div_ceil(60)),
    };
    let sub = tr!("In the world, where anyone near can grab it · {left}", left = left);
    section(ui, cx, index, icons::HAND_DEPOSIT, tr("Your link card is out"), Some(&sub), |ui| {
        let title = if card.title.trim().is_empty() { tr("Link card") } else { card.title.as_str() };
        let shown = card.link.strip_prefix("https://").unwrap_or(&card.link);
        widgets::field(ui, title, Some(shown.strip_prefix("www.").unwrap_or(shown)), 150.0, |ui| {
            let take = Button::new(tr("Take back")).icon(icons::X_CIRCLE).size(Size::Sm).width(150.0).hint(tr("Takes the card out of the world"));
            if take.id_salt("orbiters-home-take-back").show(ui).clicked() {
                acts.push(Act::Press(Target::TakeBack));
            }
        });
    });
}

/// Where what plays is, as a bar to drag or click along (where the app lets others move in its timeline), the time at
/// either end; an app that does not says so under it.
fn timeline(ui: &mut Ui, media: &Media, acts: &mut Vec<Act>) {
    let Some(length) = media.duration.filter(|d| *d > 0) else { return };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let label_w = 58.0;
    let bar = Rect::from_min_max(pos2(row.left() + label_w, row.center().y - 3.0), pos2(row.right() - label_w, row.center().y + 3.0));
    let id = Id::unique("orbiters-home-timeline");
    let sense = if media.can_seek { Sense::click_and_drag() } else { Sense::hover() };
    let resp = ui.interact(bar.expand2(vec2(10.0, 12.0)), id, sense);
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos()).map(|p| ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0));
    let held = media.can_seek && (resp.dragged() || resp.is_pointer_button_down_on());
    let at = media.position.unwrap_or(0).min(length);
    let fraction = match (held, pointer) {
        (true, Some(f)) => f,
        _ => at as f32 / length as f32,
    };
    if media.can_seek && (resp.drag_stopped() || resp.clicked()) {
        if let Some(f) = pointer {
            acts.push(Act::Seek((f * length as f32).round() as u32));
        }
    }
    if media.can_seek {
        resp.clone().on_hover_cursor(CursorIcon::PointingHand);
    }
    let hover = anim::toggle(ui.ctx(), id.with("hover"), media.can_seek && (resp.hovered() || held), Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
    let filled = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * fraction, bar.max.y));
    p.rect_filled(filled, theme::radius(3.0), if media.can_seek { theme::accent() } else { theme::fg_3() });
    if media.can_seek {
        p.circle_filled(pos2(filled.right(), bar.center().y), 4.0 + 3.0 * hover, theme::fg());
    }
    let shown_at = if held { (fraction * length as f32).round() as u64 } else { at as u64 };
    let time = |s: u64| Txt::new(orbiters_n4n0_core::menu::clock(s), 12.0).color(theme::fg_3());
    widgets::text(p, pos2(row.left(), row.center().y), Align2::LEFT_CENTER, &time(shown_at));
    widgets::text(p, pos2(row.right(), row.center().y), Align2::RIGHT_CENTER, &time(length as u64));
    if !media.can_seek {
        let app = if media.app.is_empty() { tr("This player") } else { media.app.as_str() };
        let (note, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let txt = Txt::new(tr!("{app} lets no app move in its timeline", app = app), 12.0).color(theme::fg_4());
        widgets::text(ui.painter(), pos2(bar.left(), note.center().y), Align2::LEFT_CENTER, &txt);
    }
}

fn suggestions(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let shown: Vec<&str> = menu.shown_suggestions().collect();
    if shown.is_empty() {
        let text = tr("Type what you want to watch and press Enter. A result's Copy link puts it in the clipboard, ready to paste in the world's video player.");
        return note_card(ui, cx, 2, icons::YOUTUBE_LOGO, tr("Search YouTube"), text);
    }
    section(ui, cx, 2, icons::MAGNIFYING_GLASS, tr("Suggestions"), None, |ui| {
        ui.add_space(4.0);
        for (i, text) in shown.iter().enumerate() {
            let (r, hover, pressed) = press_row(ui, Id::unique(("orbiters-home-suggestion", i)), 44.0);
            if pressed {
                acts.push(Act::Press(Target::Suggestion(i as u8)));
            }
            let p = ui.painter();
            widgets::icon(p, pos2(r.left() + 8.0, r.center().y), icons::MAGNIFYING_GLASS, 14.0, theme::fg_4());
            let line = Txt::new(*text, 13.5).color(theme::mix(theme::fg_2(), theme::fg(), hover));
            widgets::text_truncated(p, pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, &line, r.width() - 40.0);
        }
    });
}

fn results(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, copied: Option<&str>, acts: &mut Vec<Act>) {
    let sub = layout::text(menu, Content::ResultsTitle);
    // As wide as the page: the results go side by side on wide ones.
    section_up_to(ui, cx, f32::INFINITY, 2, icons::YOUTUBE_LOGO, tr("Results"), Some(&sub), |ui| {
        ui.add_space(4.0);
        if menu.loading() {
            row_columns(ui, "orbiters-home-results", ROW_MIN_W, ROW_GAP, 3, |ui, _, first| {
                if !first {
                    widgets::separator(ui);
                }
                placeholder_row(ui);
            });
            return;
        }
        let results = menu.page_results();
        row_columns(ui, "orbiters-home-results", ROW_MIN_W, ROW_GAP, results.len(), |ui, i, first| {
            if !first {
                widgets::separator(ui);
            }
            let video = &results[i];
            // After its Copy link, the row offers Drop (the avatar's screen shows the card in the world), and says it was
            // copied as long as it does.
            let drop = !layout::target_state(menu, Target::Drop(i as u8)).hidden;
            if let Some(target) = result_row(ui, i, video, picture(&video.id), drop || copied == Some(video.id.as_str()), drop) {
                acts.push(Act::Press(target));
            }
        });
        if menu.has_previous() || menu.has_next() {
            widgets::separator(ui);
            if let Some(target) = pager(ui, menu) {
                acts.push(Act::Press(target));
            }
        }
    });
}

/// A result: its thumbnail and length, its title and channel, its Copy link button and, once copied where a card can be
/// dropped, its Drop; the one clicked.
fn result_row(ui: &mut Ui, i: usize, video: &Video, texture: Option<TextureId>, copied: bool, drop: bool) -> Option<Target> {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), THUMB.y + 28.0), Sense::hover());
    let thumb = Rect::from_min_size(pos2(row.left(), row.center().y - THUMB.y / 2.0), THUMB);
    let button_w = 124.0;
    let drop_w = if drop { 96.0 + 8.0 } else { 0.0 };
    let x = thumb.right() + 16.0;
    let text_w = (row.right() - button_w - drop_w - 20.0 - x).max(80.0);
    {
        let p = ui.painter();
        thumbnail(p, thumb, texture, 10.0);
        // A badge in capitals, as the English one is.
        let live = tr("LIVE").to_uppercase();
        let length = if video.live { Some((live.as_str(), theme::REC, Color32::WHITE)) } else { video.duration.as_deref().map(|d| (d, theme::black(0.7), theme::fg())) };
        if let Some((label, bg, fg)) = length {
            super::sessions::badge(p, ui, pos2(thumb.right() - 6.0, thumb.bottom() - 28.0), label, bg, fg, true);
        }
        let mut job = Txt::new(&video.title, 14.0).weight(weight::MEDIUM).line_height(19.0).job(text_w);
        job.wrap.max_rows = 2;
        let title = p.layout_job(job);
        let title_h = title.size().y;
        let top = row.center().y - (title_h + 22.0) / 2.0;
        p.galley(pos2(x, top), title, theme::fg());
        widgets::text_truncated(p, pos2(x, top + title_h + 14.0), Align2::LEFT_CENTER, &Txt::new(&video.channel, 12.5).color(theme::fg_3()), text_w);
    }
    let (label, glyph) = if copied { (tr("Copied"), icons::CHECK) } else { (tr("Copy link"), icons::COPY) };
    let copy = Button::new(label).icon(glyph).size(Size::Sm).width(button_w).id_salt(("orbiters-home-copy", i));
    let mut clicked = None;
    if copy.show_at(ui, Rect::from_min_size(pos2(row.right() - button_w, row.center().y - 16.0), vec2(button_w, 32.0))).clicked() {
        clicked = Some(Target::Copy(i as u8));
    }
    if drop {
        let hint = tr("Drop it in the world as a card: whoever grabs it copies the link");
        let button = Button::new(tr("Drop")).icon(icons::HAND_DEPOSIT).size(Size::Sm).variant(Variant::Primary).width(96.0).hint(hint);
        let at = Rect::from_min_size(pos2(row.right() - button_w - 8.0 - 96.0, row.center().y - 16.0), vec2(96.0, 32.0));
        if button.id_salt(("orbiters-home-drop", i)).show_at(ui, at).clicked() {
            clicked = Some(Target::Drop(i as u8));
        }
    }
    clicked
}

/// A result's place while the search runs.
fn placeholder_row(ui: &mut Ui) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), THUMB.y + 28.0), Sense::hover());
    let p = ui.painter();
    let thumb = Rect::from_min_size(pos2(row.left(), row.center().y - THUMB.y / 2.0), THUMB);
    p.rect_filled(thumb, theme::radius(10.0), theme::surface_soft());
    let x = thumb.right() + 16.0;
    for (dy, w) in [(-14.0, ((row.right() - x) * 0.55).max(60.0)), (10.0, 120.0)] {
        p.rect_filled(Rect::from_min_size(pos2(x, row.center().y + dy - 5.0), vec2(w, 10.0)), theme::radius(5.0), theme::white(0.06));
    }
}

/// The results' pages: the one before, where it is, the next; the one pressed.
fn pager(ui: &mut Ui, menu: &Menu) -> Option<Target> {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    let label = Txt::new(tr!("Page {page}", page = layout::text(menu, Content::PageLabel)), 12.5).color(theme::fg_3());
    widgets::text(ui.painter(), row.center(), Align2::CENTER_CENTER, &label);
    let mut pressed = None;
    for (target, glyph, name, dx, on) in [
        (Target::Previous, icons::CARET_LEFT, mark("Previous page"), -70.0, menu.has_previous()),
        (Target::Next, icons::CARET_RIGHT, mark("Next page"), 70.0, menu.has_next()),
    ] {
        if icon_button(ui, Rect::from_center_size(row.center() + vec2(dx, 0.0), Vec2::splat(36.0)), glyph, tr(name), on) {
            pressed = Some(target);
        }
    }
    pressed
}

fn call(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let sub = if menu.calls_ready { layout::text(menu, Content::CallsTitle) } else { tr("Friends who are online, a press away").to_string() };
    top(ui, cx, tr_ctx("voice call", "Call"), &sub, acts);
    if let Some(call) = &menu.call {
        let ringing = matches!(call, Call::Ringing { .. });
        let name = layout::text(menu, Content::CallName);
        let state = layout::text(menu, Content::CallState);
        section(ui, cx, 1, icons::PHONE, &name, Some(&state), |ui| {
            ui.add_space(14.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            let mut x = row.left();
            if ringing {
                let answer = Button::new(tr("Answer")).icon(icons::PHONE).variant(Variant::Primary).id_salt(("orbiters-home-answer", menu.call_number));
                let w = answer.desired_width(ui);
                if answer.confirm_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))) {
                    acts.push(Act::Press(Target::Answer));
                }
                x += w + 8.0;
            }
            let end = Button::new(if ringing { tr("Decline") } else { tr("Hang up") }).icon(icons::PHONE_X).variant(Variant::Danger).id_salt(("orbiters-home-hang-up", menu.call_number));
            let w = end.desired_width(ui);
            if end.confirm_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))) {
                acts.push(Act::Press(Target::HangUp));
            }
            ui.add_space(8.0);
        });
        return;
    }
    if !menu.calls_ready {
        return note_card(ui, cx, 1, icons::PHONE, tr("Calls are not available"), tr(&menu.call_note));
    }
    if menu.contacts.is_empty() {
        return note_card(ui, cx, 1, icons::USERS, tr("No friend online"), tr("Friends who can take a call show here as soon as they are online."));
    }
    section(ui, cx, 1, icons::USERS, tr("Friends"), Some(tr("Online, and able to take a call")), |ui| {
        ui.add_space(4.0);
        for (i, contact) in menu.contacts.iter().enumerate() {
            if i > 0 {
                widgets::separator(ui);
            }
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
            let call = Button::new(tr("Call")).icon(icons::PHONE).size(Size::Sm).id_salt(("orbiters-home-call", i));
            let call_w = call.desired_width(ui);
            {
                let p = ui.painter();
                let face = pos2(row.left() + 18.0, row.center().y);
                p.circle_filled(face, 18.0, theme::ink_750());
                widgets::icon(p, face, icons::USER, 17.0, theme::fg_3());
                let name = Txt::new(&contact.name, 13.5).weight(weight::MEDIUM);
                widgets::text_truncated(p, pos2(row.left() + 48.0, row.center().y), Align2::LEFT_CENTER, &name, row.width() - 48.0 - call_w - 16.0);
            }
            if call.show_at(ui, Rect::from_min_size(pos2(row.right() - call_w, row.center().y - 16.0), vec2(call_w, 32.0))).clicked() {
                acts.push(Act::Call(i));
            }
        }
    });
}

fn photo(ui: &mut Ui, cx: &Cx, menu: &Menu, blocked: Option<&str>, acts: &mut Vec<Act>) {
    // What stopped the last picture ("Open the camera before") shows by the buttons, plainly.
    let warning = menu.notice.as_deref().filter(|_| menu.countdown.is_none()).map(tr);
    let sub = match menu.countdown {
        Some(n) => tr!("Starting in {n}… hold still", n = n),
        None => tr("VRChat's camera flies, then comes back where it was").to_string(),
    };
    top(ui, cx, tr("3D picture"), &sub, acts);
    // The two pictures, as on the Capture page.
    let w = ui.available_width();
    let gap = 16.0;
    let card_w = (w - gap) / 2.0;
    let (row, _) = ui.allocate_exact_size(vec2(w, 112.0), Sense::hover());
    let takes = [
        (PhotoMode::People, icons::USERS, theme::VIOLET, mark("3D picture of people"), mark("Goes around the people here")),
        (PhotoMode::World, icons::GLOBE_HEMISPHERE_WEST, theme::accent(), mark("3D picture of the world"), mark("Goes through the place, avatars left out")),
    ];
    for (i, (mode, glyph, tint, title, note)) in takes.into_iter().enumerate() {
        let rect = Rect::from_min_size(row.min + vec2(i as f32 * (card_w + gap), 0.0), vec2(card_w, 112.0));
        let (rect, alpha) = super::entering(ui, cx, 1 + i, rect);
        let enabled = menu.countdown.is_none() && blocked.is_none();
        let id = Id::unique(("orbiters-home-take", i));
        let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
        widgets::note_control(ui.ctx(), title, rect, enabled);
        let hover = anim::toggle(ui.ctx(), id.with("hover"), enabled && resp.hovered(), Spring::SOFT);
        let press = anim::toggle(ui.ctx(), id.with("press"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
        if enabled && widgets::pressed(&resp) {
            widgets::note_press(title);
            acts.push(Act::Press(Target::TakeOf(mode)));
        }
        if enabled {
            resp.on_hover_cursor(CursorIcon::PointingHand);
        } else if let Some(why) = blocked {
            resp.on_hover_text(why);
        }
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.02 * press));
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha * if enabled { 1.0 } else { 0.5 });
        widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), tint, 0.10 + 0.08 * hover));
        let chip = Rect::from_min_size(pos2(r.left() + 22.0, r.center().y - 26.0), vec2(52.0, 52.0));
        p.rect_filled(chip, theme::radius(16.0), theme::alpha(tint, 0.18));
        widgets::icon(&p, chip.center(), glyph, 26.0, tint);
        let x = chip.right() + 18.0;
        widgets::text_truncated(&p, pos2(x, r.center().y - 12.0), Align2::LEFT_CENTER, &Txt::new(tr(title), 19.0).weight(weight::SEMIBOLD), r.right() - x - 16.0);
        widgets::text_truncated(&p, pos2(x, r.center().y + 14.0), Align2::LEFT_CENTER, &Txt::new(tr(note), 13.5).color(theme::fg_2()), r.right() - x - 16.0);
    }
    if let Some(warning) = warning {
        let galley = ui.painter().layout_job(Txt::new(warning, 14.0).weight(weight::SEMIBOLD).color(theme::WARN).line_height(20.0).job(w - 28.0));
        let (row, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
        let p = ui.painter();
        widgets::icon(p, pos2(row.left() + 9.0, row.top() + 10.0), icons::WARNING, 18.0, theme::WARN);
        p.galley(pos2(row.left() + 28.0, row.top()), galley, theme::WARN);
    }
    section(ui, cx, 3, icons::CUBE_FOCUS, tr("Camera"), Some(tr("The timer and the flight stay as chosen")), |ui| {
        let mut timer = menu.photo_timer;
        let timers: Vec<SegOption<u32>> = TIMERS.iter().zip(strings::TIMERS).map(|(&t, label)| SegOption::new(t, menu_text(label))).collect();
        widgets::field(ui, tr("Timer"), Some(tr("Time to get ready after you press.")), 240.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-timer", &mut timer, &timers, true) {
                if let Some(i) = TIMERS.iter().position(|&t| t == timer) {
                    acts.push(Act::Press(Target::Timer(i as u8)));
                }
            }
        });
        widgets::separator(ui);
        let mut length = menu.photo_length;
        let lengths: Vec<SegOption<PhotoLength>> = PhotoLength::ALL
            .iter()
            .zip(super::capture::flight_labels())
            .zip(super::capture::FLIGHT_HINTS)
            .map(|((&l, label), hint)| SegOption::new(l, label).hint(tr(hint)))
            .collect();
        widgets::field(ui, tr("Camera flight"), Some(tr("Longer flights see more.")), 200.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-length", &mut length, &lengths, true) {
                if let Some(i) = PhotoLength::ALL.iter().position(|&l| l == length) {
                    acts.push(Act::Press(Target::Length(i as u8)));
                }
            }
        });
    });
}

fn music(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, copied: Option<&str>, acts: &mut Vec<Act>) {
    let media = menu.media.as_ref().filter(|m| !m.title.is_empty());
    let sub = match media {
        Some(_) => layout::text(menu, Content::MediaState),
        None => tr("What this computer plays, and its volume").to_string(),
    };
    top(ui, cx, tr("Music"), &sub, acts);
    let from = media.map(|m| m.app.as_str()).filter(|app| !app.is_empty()).map(|app| tr!("From {app}", app = app));
    let mut m = super::Masonry::new(ui, "home-music");
    m.add(1, |ui| {
        section(ui, cx, 1, icons::MUSIC_NOTES, tr("Now playing"), from.as_deref(), |ui| {
            ui.add_space(14.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 108.0), Sense::hover());
            let art = Rect::from_min_size(row.min, vec2(192.0, 108.0));
            let controls_w = 40.0 + 10.0 + 48.0 + 10.0 + 40.0;
            {
                let p = ui.painter();
                match media.and_then(|m| m.art.as_deref()).and_then(picture) {
                    Some(texture) => thumbnail(p, art, Some(texture), 12.0),
                    None => {
                        thumbnail(p, art, None, 12.0);
                        widgets::icon(p, art.center(), icons::MUSIC_NOTES, 30.0, theme::fg_4());
                    }
                }
                let x = art.right() + 20.0;
                let text_w = (row.right() - x - if media.is_some() { controls_w + 24.0 } else { 0.0 }).max(80.0);
                let lines = [
                    (Content::MediaTitle, 26.0, Txt::new("", 17.0).weight(weight::SEMIBOLD)),
                    (Content::MediaArtist, 52.0, Txt::new("", 13.0).color(theme::fg_2())),
                    (Content::MediaAlbum, 74.0, Txt::new("", 12.5).color(theme::fg_3())),
                ];
                for (content, dy, style) in lines {
                    let line = Txt { text: layout::text(menu, content), ..style };
                    widgets::text_truncated(p, pos2(x, row.top() + dy), Align2::LEFT_CENTER, &line, text_w);
                }
            }
            if let Some(m) = media {
                let (cy, mut x) = (row.center().y, row.right() - controls_w);
                // Previous, or back to the start of what has nothing before it.
                let previous = !layout::target_state(menu, Target::MediaPrevious).hidden;
                if icon_button(ui, Rect::from_min_size(pos2(x, cy - 20.0), Vec2::splat(40.0)), icons::SKIP_BACK, tr("Previous"), previous) {
                    acts.push(Act::Press(Target::MediaPrevious));
                }
                x += 50.0;
                let play = Button::new("")
                    .icon(if m.playing { icons::PAUSE } else { icons::PLAY })
                    .variant(Variant::Primary)
                    .size(Size::Lg)
                    .width(48.0)
                    .enabled(m.can_play_pause)
                    .hint(if m.playing { tr("Pause") } else { tr("Play") })
                    .id_salt("orbiters-home-play");
                if play.show_at(ui, Rect::from_min_size(pos2(x, cy - 24.0), Vec2::splat(48.0))).clicked() {
                    acts.push(Act::Press(Target::MediaPlayPause));
                }
                x += 58.0;
                if icon_button(ui, Rect::from_min_size(pos2(x, cy - 20.0), Vec2::splat(40.0)), icons::SKIP_FORWARD, tr("Next"), m.can_next) {
                    acts.push(Act::Press(Target::MediaNext));
                }
            }
            if let Some(m) = media.filter(|m| m.duration.is_some_and(|d| d > 0)) {
                ui.add_space(8.0);
                timeline(ui, m, acts);
            }
            ui.add_space(14.0);
            // The YouTube video a browser or YouTube's app plays, found on YouTube: its link copied, then dropped as a card,
            // as a search result's.
            if let Some((video, link)) = media.and_then(|m| Some((m.video.as_deref()?, m.link()?))) {
                widgets::separator(ui);
                let drop = !layout::target_state(menu, Target::DropPlaying).hidden;
                let was_copied = drop || copied == Some(video);
                let width = 124.0 + if drop { 8.0 + 96.0 } else { 0.0 };
                let shown = link.strip_prefix("https://www.").unwrap_or(&link);
                widgets::field(ui, tr("On YouTube"), Some(shown), width, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                    let (label, glyph) = if was_copied { (tr("Copied"), icons::CHECK) } else { (tr("Copy link"), icons::COPY) };
                    let copy = Button::new(label).icon(glyph).size(Size::Sm).width(124.0).id_salt("orbiters-home-copy-playing");
                    if copy.hint(tr("Paste it in a world's video player")).show(ui).clicked() {
                        acts.push(Act::Press(Target::CopyPlaying));
                    }
                    if drop {
                        let hint = tr("Drop it in the world as a card: whoever grabs it copies the link");
                        let button = Button::new(tr("Drop")).icon(icons::HAND_DEPOSIT).size(Size::Sm).variant(Variant::Primary).width(96.0).hint(hint);
                        if button.id_salt("orbiters-home-drop-playing").confirm(ui) {
                            acts.push(Act::Press(Target::DropPlaying));
                        }
                    }
                });
            }
            // The app playing alone, where Windows gives each app a volume (Enzo, 2026-11-16); the speakers where apps have none.
            let app = media.map(|m| m.app.as_str()).filter(|app| !app.is_empty());
            let volume = match menu.playing_volume() {
                Some(PlayingVolume::App(volume)) => Some((volume, app.map_or_else(|| tr("Only this app: other sounds stay as they are.").to_string(), |app| tr!("Only {app}: other sounds stay as they are.", app = app)))),
                Some(PlayingVolume::Computer(volume)) => Some((volume, tr("This computer's speakers.").to_string())),
                Some(PlayingVolume::NotFound) => {
                    widgets::separator(ui);
                    let why = app.map_or_else(|| tr("No sound from this app to set yet.").to_string(), |app| tr!("No sound from {app} to set yet.", app = app));
                    widgets::field(ui, tr("Volume"), Some(&why), 340.0, |ui| {
                        // Off, and nothing set in its place: not the computer's volume, not another app's.
                        ui.add_enabled_ui(false, |ui| {
                            let mut none = 0.0;
                            widgets::slider(ui, "orbiters-home-volume", &mut none, 0.0..=100.0, 1.0, |_| "\u{2013}".to_string());
                        });
                    });
                    None
                }
                None => None,
            };
            if let Some((volume, says)) = volume {
                widgets::separator(ui);
                widgets::field(ui, tr("Volume"), Some(&says), 340.0, |ui| {
                    let mut percent = volume.percent as f32;
                    let format = |v: f32| if volume.muted { tr("Muted").to_string() } else { format!("{v:.0} %") };
                    if widgets::slider(ui, "orbiters-home-volume", &mut percent, 0.0..=100.0, 1.0, format) {
                        acts.push(Act::Volume(percent.round() as u8));
                    }
                    ui.add_space(6.0);
                    let (glyph, label) = if volume.muted { (icons::SPEAKER_X, tr("Unmute")) } else { (icons::SPEAKER_HIGH, tr("Mute")) };
                    if IconButton::new(glyph, label).show(ui).clicked() {
                        acts.push(Act::Press(Target::Mute));
                    }
                });
            }
        })
    });
    m.add(2, |ui| card_out(ui, cx, 2, acts));
    // What the world's video player plays (VRChat's log): watched here from where the world is, or saved.
    if let Some(video) = &menu.world_video {
        let title = video.title.clone().unwrap_or_else(|| video.url.clone());
        m.add(3, |ui| {
            section(ui, cx, 3, icons::MONITOR_PLAY, tr("In the world"), Some(tr("What the world's video player plays now")), |ui| {
                widgets::field(ui, &title, Some(&video.url), 260.0, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                    if Button::new(tr("Save")).icon(icons::FLOPPY_DISK).size(Size::Sm).show(ui).clicked() {
                        acts.push(Act::Press(Target::SaveVideo));
                    }
                    if Button::new(tr("Watch"))
                        .icon(icons::MONITOR_PLAY)
                        .variant(Variant::Primary)
                        .size(Size::Sm)
                        .hint(tr("Opens it in your browser, where the world's player is"))
                        .show(ui)
                        .clicked()
                    {
                        acts.push(Act::Press(Target::WatchVideo));
                    }
                });
            })
        });
    }
    m.finish(ui);
}

fn light(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let Some(light) = menu.light else {
        top(ui, cx, tr("Light"), tr("N4N0 as a light everyone around sees"), acts);
        return note_card(ui, cx, 1, icons::LIGHTBULB, tr("No light on this N4N0"), &layout::text(menu, Content::LightNote));
    };
    top(ui, cx, tr("Light"), &layout::text(menu, Content::LightState), acts);
    section(ui, cx, 1, icons::LIGHTBULB, tr("N4N0's light"), Some(&layout::text(menu, Content::LightNote)), |ui| {
        widgets::field(ui, tr("Light"), None, 48.0, |ui| {
            let mut on = light.on;
            if widgets::toggle(ui, "orbiters-home-light", &mut on) {
                acts.push(Act::Press(Target::LightSwitch));
            }
        });
        widgets::separator(ui);
        let name = strings::LIGHT_COLORS.get(light.color as usize).map_or("", |&t| menu_text(t));
        widgets::field(ui, tr("Colour"), Some(name), swatches_width(), |ui| {
            if let Some(i) = swatches(ui, light.color) {
                acts.push(Act::Press(Target::LightColor(i)));
            }
        });
        widgets::separator(ui);
        let mut level = light.level_choice();
        let levels: Vec<SegOption<usize>> = (0..LIGHT_LEVELS.len()).map(|i| SegOption::new(i, strings::LIGHT_LEVELS.get(i).map_or("", |&t| menu_text(t)))).collect();
        widgets::field(ui, tr_ctx("lamp", "Brightness"), None, 260.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-light-level", &mut level, &levels, true) {
                acts.push(Act::Press(Target::LightLevel(level as u8)));
            }
        });
    });
}

/// Tracking smoothing: the switch, each kind of device's time constant in milliseconds, and what the driver does now.
fn tracking(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let Some(tracking) = menu.tracking.clone() else {
        top(ui, cx, tr("Tracking"), tr("Smoother trackers, controllers and headset"), acts);
        return note_card(ui, cx, 1, icons::VIRTUAL_REALITY, tr("Not on this computer"), &layout::text(menu, Content::TrackingNote));
    };
    top(ui, cx, tr("Tracking"), &tracking.note, acts);
    let sub = tr("Less jitter, a little more lag: a device in steady motion trails by the milliseconds you set");
    section(ui, cx, 1, icons::VIRTUAL_REALITY, tr("Smooth tracking"), Some(sub), |ui| {
        widgets::field(ui, tr("Smoothing"), Some(tr("Orbiters - N4N0's SteamVR driver: on, off and strengths at once")), 48.0, |ui| {
            let mut on = tracking.on;
            if widgets::toggle(ui, "orbiters-home-tracking", &mut on) {
                acts.push(Act::Press(Target::TrackingSwitch));
            }
        });
        let hints = [mark("Keep it low: a smoothed headset lags behind your head"), mark("For your hands"), mark("Full-body trackers jitter most")];
        for (k, kind) in strings::TRACKING_KINDS.iter().enumerate() {
            widgets::separator(ui);
            let mut ms = tracking.ms[k] as f32;
            widgets::field(ui, menu_text(*kind), Some(tr(hints[k])), 320.0, |ui| {
                let max = orbiters_core::settings::TrackingSettings::MAX_MS as f32;
                if widgets::slider(ui, format!("orbiters-home-smoothing-{k}"), &mut ms, 0.0..=max, 1.0, smoothing_words) {
                    acts.push(Act::Smoothing(k, ms.round() as u16));
                }
            });
        }
    });
}

/// A smoothing slider's value.
fn smoothing_words(ms: f32) -> String {
    if ms < 0.5 {
        tr("Off").to_string()
    } else {
        tr!("{ms} ms", ms = format!("{ms:.0}"))
    }
}

/// Suggest features: the idea written here, sent to the Orbiters team.
fn suggest(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    top(ui, cx, tr("Suggest a feature"), tr("What should N4N0 do next? The Orbiters team reads every idea"), acts);
    let note = layout::text(menu, Content::SuggestNote);
    section(ui, cx, 1, icons::CHAT_TEARDROP_TEXT, tr("Your idea"), (!note.is_empty()).then_some(note.as_str()), |ui| {
        ui.add_space(4.0);
        let mut text = menu.suggestion.clone();
        let w = ui.available_width();
        let frame = egui::Frame::NONE.fill(theme::control()).corner_radius(theme::radius(12.0)).inner_margin(egui::Margin::same(12));
        frame.show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut text)
                .id_salt("orbiters-home-suggestion")
                .hint_text(tr("A feature, a fix, anything N4N0 or Orbiters - N4N0 could do better"))
                .frame(egui::Frame::NONE)
                .desired_rows(5)
                .desired_width(w - 24.0)
                .char_limit(orbiters_n4n0_core::menu::SUGGESTION_MAX)
                .font(egui::FontId::proportional(14.0));
            ui.add(edit);
        });
        if text != menu.suggestion {
            acts.push(Act::Suggestion(text.clone()));
        }
        ui.add_space(12.0);
        let ready = !text.trim().is_empty() && !menu.sending;
        let label = if menu.sending { tr("Sending…") } else { tr("Send") };
        let button = Button::new(label).icon(icons::PAPER_PLANE_TILT).variant(Variant::Primary).enabled(ready);
        let bw = button.desired_width(ui);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
        if button.confirm_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 36.0))) {
            acts.push(Act::Press(Target::SuggestSend));
        }
    });
    // The beta's menu mismatch buttons, the same setting as Settings › Beta feedback's switch (Enzo, 2026-11-16).
    if let Some(on) = menu.mismatch_buttons {
        ui.add_space(12.0);
        section(ui, cx, 2, icons::CHAT_CIRCLE_TEXT, tr("Beta feedback"), None, |ui| {
            widgets::field(
                ui,
                tr("Menu mismatch buttons"),
                Some(tr("Three buttons, on your wrist in VR and in a corner of VRChat's window on the desktop, to press when N4N0's menus do not match VRChat's; each press keeps a report on this computer, never what you type or say.")),
                48.0,
                |ui| {
                    let mut shown = on;
                    if widgets::toggle(ui, "home-mismatch-buttons", &mut shown) {
                        acts.push(Act::Press(Target::MismatchButtons));
                    }
                },
            );
        });
    }
}

/// The QR code scanner: what it does now (VRChat's stream camera is read while this page shows), how to turn the stream
/// camera on, and the last link it opened with Copy link and Open again.
fn qr(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    top(ui, cx, tr("QR code scanner"), tr("Point VRChat's stream camera at a QR code: its web link opens in your browser"), acts);
    let scanner = &menu.qr;
    let note = layout::qr_note(scanner);
    let mut m = super::Masonry::new(ui, "home-qr");
    m.add(1, |ui| {
        section(
            ui,
            cx,
            1,
            icons::QR_CODE,
            tr("VRChat's stream camera"),
            Some(tr("Read about three times a second while this page shows, for two minutes at most")),
            |ui| {
                use orbiters_n4n0_core::qr::QrPhase;
                let tone = match scanner.phase {
                    QrPhase::Looking => Tone::Ok,
                    QrPhase::Starting => Tone::Info,
                    QrPhase::NoCamera => Tone::Warn,
                    QrPhase::Off | QrPhase::Stopped => Tone::Off,
                };
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
                let p = ui.painter();
                widgets::status_dot(p, pos2(row.left() + 5.0, row.top() + 20.0), tone, scanner.scanning());
                let state = Txt::new(layout::qr_state(scanner), 15.0).weight(weight::SEMIBOLD);
                widgets::text(p, pos2(row.left() + 20.0, row.top() + 20.0), Align2::LEFT_CENTER, &state);
                let room = row.width() - 200.0;
                widgets::text_truncated(p, pos2(row.left() + 20.0, row.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(&note, 12.5).color(theme::fg_3()), room);
                if !scanner.scanning() {
                    let button = Button::new(tr("Scan again")).icon(icons::QR_CODE).variant(Variant::Primary);
                    let bw = button.desired_width(ui);
                    if button.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.center().y - 18.0), vec2(bw, 36.0))).clicked() {
                        acts.push(Act::Press(Target::QrAgain));
                    }
                }
            },
        )
    });
    let link = scanner.link.as_deref();
    let shown = link.map_or_else(|| tr("None yet").to_string(), |l| layout::short_link(l, 90));
    let policy = tr("Only http and https links open, each once while it stays before the camera");
    let notice = menu.notice.clone();
    m.add(2, |ui| {
        section(ui, cx, 2, icons::LINK_SIMPLE, tr("Last link opened"), Some(notice.as_deref().map_or(policy, tr)), |ui| {
            widgets::field(ui, &shown, None, 260.0, |ui| {
                if Button::new(tr("Open again")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).enabled(link.is_some()).show(ui).clicked() {
                    acts.push(Act::Press(Target::QrOpen));
                }
                ui.add_space(8.0);
                if Button::new(tr("Copy link")).icon(icons::COPY).variant(Variant::Secondary).enabled(link.is_some()).show(ui).clicked() {
                    acts.push(Act::Press(Target::QrCopy));
                }
            });
        })
    });
    m.add(3, |ui| {
        note_card(
            ui,
            cx,
            3,
            icons::INFO,
            tr("Turning the stream camera on"),
            tr("In VRChat, open the camera, choose Stream, then switch Spout Stream on in the camera's settings. Settings › Spout sender picks another sender."),
        )
    });
    m.finish(ui);
}

/// Previous instances: Invite me sends the player an invite back to one, from their own VRChat account when it is
/// connected here (`own_vrchat`, REQUEST20 4), else from Orbiters' VRChat account once it is their friend (Add Orbiters as
/// a friend, with its name and page, shows above the list when Orbiters asked for it). When only their own account can
/// do it, Connect VRChat shows above the list, with why.
fn instances(ui: &mut Ui, cx: &Cx, menu: &Menu, own_vrchat: bool, acts: &mut Vec<Act>) {
    top(ui, cx, tr("Previous instances"), &layout::text(menu, Content::InstancesTitle), acts);
    let mut index = 1;
    if let Some(why) = menu.invite_vrchat.as_deref().filter(|_| !menu.invite_waiting) {
        section_up_to(ui, cx, f32::INFINITY, index, icons::SIGN_IN, tr("Invite me needs your VRChat account"), Some(why), |ui| {
            if Button::new(tr("Connect VRChat")).icon(icons::SIGN_IN).variant(Variant::Primary).show(ui).clicked() {
                acts.push(Act::VrchatAccount);
            }
        });
        index += 1;
    }
    if let Some(account) = &menu.invite_account {
        let why = tr("VRChat lets Orbiters invite only its friends: in VRChat, Social, search the name, Add friend; then Try again");
        section_up_to(ui, cx, f32::INFINITY, index, icons::USER, tr("Add Orbiters as a friend"), Some(why), |ui| {
            let link = layout::short_link(&account.link, 70);
            widgets::field(ui, &account.name, Some(&link), 420.0, |ui| {
                let again =
                    Button::new(tr("Try again")).icon(icons::PAPER_PLANE_TILT).variant(Variant::Primary).enabled(!menu.invite_waiting && menu.inviting.is_some());
                if again.show(ui).clicked() {
                    acts.push(Act::Press(Target::InviteAgain));
                }
                ui.add_space(8.0);
                if Button::new(tr("Open link")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).show(ui).clicked() {
                    acts.push(Act::Press(Target::OpenFriendLink));
                }
                ui.add_space(8.0);
                if Button::new(tr("Copy name")).icon(icons::COPY).variant(Variant::Secondary).show(ui).clicked() {
                    acts.push(Act::Press(Target::CopyFriendName));
                }
            });
        });
        index += 1;
    }
    if menu.instances.is_empty() {
        let text = if menu.instances_ready { tr("Worlds you join in VRChat show here, newest first.") } else { tr("Reading VRChat's logs…") };
        return note_card(ui, cx, index, icons::GLOBE, tr("No instance yet"), text);
    }
    let sub = if own_vrchat {
        tr("Your VRChat account invites you back: look at your VRChat notifications")
    } else {
        tr("Orbiters' VRChat account invites you back: open the invite in VRChat")
    };
    // As wide as the page: the instances go side by side on wide ones.
    section_up_to(ui, cx, f32::INFINITY, index, icons::GLOBE, tr("Instances you were in"), Some(sub), |ui| {
        ui.add_space(4.0);
        row_columns(ui, "orbiters-home-instances", ROW_MIN_W, ROW_GAP, menu.page_instances().len(), |ui, i, first| {
            if !first {
                widgets::separator(ui);
            }
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
            let invite =
                Button::new(tr("Invite me")).icon(icons::PAPER_PLANE_TILT).size(Size::Sm).enabled(!menu.invite_waiting).id_salt(("orbiters-home-invite", i));
            let invite_w = invite.desired_width(ui);
            {
                let p = ui.painter();
                let room = r.width() - invite_w - 16.0;
                let world = Txt::new(layout::text(menu, Content::InstanceWorld(i as u8)), 13.5).weight(weight::MEDIUM);
                widgets::text_truncated(p, pos2(r.left(), r.center().y - 9.0), Align2::LEFT_CENTER, &world, room);
                let meta = Txt::new(layout::text(menu, Content::InstanceMeta(i as u8)), 12.0).color(theme::fg_3());
                widgets::text_truncated(p, pos2(r.left(), r.center().y + 11.0), Align2::LEFT_CENTER, &meta, room);
            }
            if invite.confirm_at(ui, Rect::from_min_size(pos2(r.right() - invite_w, r.center().y - 16.0), vec2(invite_w, 32.0))) {
                acts.push(Act::Press(Target::InviteMe(i as u8)));
            }
        });
        if menu.has_previous() || menu.has_next() {
            if let Some(target) = pager(ui, menu) {
                acts.push(Act::Press(target));
            }
        }
    });
}

const SWATCH: f32 = 26.0;
const SWATCH_GAP: f32 = 10.0;

fn swatches_width() -> f32 {
    LIGHT_COLORS.len() as f32 * (SWATCH + SWATCH_GAP) - SWATCH_GAP + 8.0
}

/// The light's colours as round swatches, the chosen one ringed; the one pressed (pressing turns the light on, the
/// chosen colour too).
fn swatches(ui: &mut Ui, chosen: u8) -> Option<u8> {
    let (rect, _) = ui.allocate_exact_size(vec2(swatches_width(), 36.0), Sense::hover());
    let ctx = ui.ctx().clone();
    let mut pressed = None;
    for (i, (_, [r, g, b])) in LIGHT_COLORS.iter().enumerate() {
        let center = pos2(rect.left() + 4.0 + SWATCH / 2.0 + i as f32 * (SWATCH + SWATCH_GAP), rect.center().y);
        let id = Id::unique(("orbiters-home-colour", i));
        let resp = ui.interact(Rect::from_center_size(center, Vec2::splat(SWATCH + 8.0)), id, Sense::click());
        if widgets::pressed(&resp) {
            pressed = Some(i as u8);
        }
        let on = anim::toggle(&ctx, id.with("on"), i as u8 == chosen, Spring::SNAPPY);
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let down = anim::toggle(&ctx, id.with("down"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let p = ui.painter();
        let ring = theme::mix(theme::white(0.18 * hover), theme::fg(), on);
        if ring.a() > 0 {
            p.circle_stroke(center, SWATCH / 2.0 + 4.0, Stroke::new(2.0, ring));
        }
        p.circle_filled(center, SWATCH / 2.0 * (1.0 - 0.1 * down), Color32::from_rgb(*r, *g, *b));
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(strings::LIGHT_COLORS.get(i).map_or("", |&t| menu_text(t)));
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Ctl};
    use crate::ipc::N4n0Command;
    use crate::state::{N4n0Light, Snapshot};
    use egui::{Event, Modifiers, PointerButton, RawInput};
    use orbiters_ui::toast::Toasts;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// What Home asked of the app: settings changes and N4N0 commands; the launcher's actions and what it asked of
    /// VRChat (REQUEST20 2: a new instance is made, as VRChat would answer, nothing reaches it).
    #[derive(Default)]
    struct Recorder {
        patches: Mutex<Vec<serde_json::Value>>,
        n4n0: Mutex<Vec<N4n0Command>>,
        launcher: Mutex<Vec<orbiters_core::launcher::Action>>,
        asked: Mutex<Vec<String>>,
        /// What VRChat says of an instance asked about (REQUEST21 7: whether it is still open).
        alive: Mutex<Option<bool>>,
        /// How long ago VRChat said it (the service kept the answer).
        age_ms: Mutex<u64>,
        /// By instance: what VRChat says of it and how long it takes to say it (verify-r21 Major 4: answers crossing).
        alive_by: Mutex<std::collections::HashMap<String, (Option<bool>, Duration)>>,
        /// By address: the picture VRChat answers with (none: it fails) and how long it takes.
        images: Mutex<std::collections::HashMap<String, (Option<Vec<u8>>, Duration)>>,
    }

    /// The instance VRChat makes in these tests.
    const MADE: &str = "48213~hidden(usr_00000000-0000-4000-8000-000000000000)~region(us)~nonce(0a1b2c3d-0000-4000-8000-000000000000)";

    impl Backend for Recorder {
        fn launcher(&self, action: orbiters_core::launcher::Action) -> bool {
            self.launcher.lock().unwrap().push(action);
            true
        }
        fn vrchat_data(&self, _scope: crate::controller::vrchat_data::Scope, request: crate::controller::vrchat_data::Request) -> crate::controller::vrchat_data::Result {
            use crate::controller::vrchat_data::{Request, Response};
            self.asked.lock().unwrap().push(format!("{request:?}"));
            match request {
                Request::CreateInstance { world, access, region } => Ok(Response::CreatedInstance(orbiters_core::vrchat_api::WorldInstance {
                    world,
                    id: MADE.into(),
                    access: if access == orbiters_core::vrchat_api::CreateAccess::FriendsPlus { orbiters_core::vrchat_api::InstanceAccess::FriendsPlus } else { orbiters_core::vrchat_api::InstanceAccess::Unknown },
                    region: format!("{region:?}").to_lowercase(),
                    occupants: 0,
                    capacity: None,
                    friends: None,
                    full: false,
                    closed: false,
                    can_join: true,
                })),
                Request::InstanceSeen { instance, .. } => {
                    let by = self.alive_by.lock().unwrap().get(&instance).copied();
                    let age_ms = *self.age_ms.lock().unwrap();
                    match by {
                        Some((open, wait)) => {
                            std::thread::sleep(wait);
                            Ok(Response::InstanceSeen { open, age_ms })
                        }
                        None => Ok(Response::InstanceSeen { open: *self.alive.lock().unwrap(), age_ms }),
                    }
                }
                Request::Image { url } => {
                    let (bytes, wait) = self.images.lock().unwrap().get(&url).cloned().unwrap_or((None, Duration::ZERO));
                    std::thread::sleep(wait);
                    bytes.map(Response::Image).ok_or_else(|| "not found".to_string())
                }
                _ => Err("not in tests".into()),
            }
        }
        fn n4n0(&self, command: N4n0Command) {
            self.n4n0.lock().unwrap().push(command);
        }
        fn patch_settings(&self, patch: serde_json::Value) {
            self.patches.lock().unwrap().push(patch);
        }
        fn view(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::new(Settings::default()))
        }
        fn take_notices(&self) -> Vec<crate::controller::Notice> {
            Vec::new()
        }
        fn library_version(&self) -> u64 {
            0
        }
        fn attach(&self, _ctx: &egui::Context) {}
        fn start_capture(&self) {}
        fn stop_capture(&self) {}
        fn pause_capture(&self, _paused: bool) {}
        fn start_test_capture(&self) {}
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn trash_session(&self, _dir: PathBuf) {}
        fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {}
        fn unqueue_reconstruction(&self, _dir: PathBuf) {}
        fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
        fn sign_in(&self) {}
        fn reopen_sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn request_worker_log(&self) {}
        fn worker_log(&self) -> Option<Vec<String>> {
            None
        }
        fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
        fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
        fn request_maps(&self) {}
        fn maps(&self) -> Option<crate::state::MapsMine> {
            None
        }
        fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
        fn frames(&self, _chunk: crate::state::FrameChunk) {}
        fn headset(&self, _in_use: bool) {}
        fn walk_boost(&self, _strength: f32) {}
        fn osc_reconnect(&self) {}
        fn updates(&self, _action: crate::ipc::UpdateAction) {}
        fn quit(&self) {}
    }

    /// Home in a 1048 × 780 page, frame by frame. On All tools (`State::open_all_tools`) its cards are four to a row,
    /// 233 wide from x 32, 128 high from y 140 (16 apart), as Home's were: fast walk, YouTube, Call, 3D picture, then
    /// music, light, States, Tracking, then suggest, previous instances, the QR code scanner and what N4N0 says.
    struct Harness {
        ctx: egui::Context,
        recorder: Arc<Recorder>,
        ctl: Ctl,
        snap: Snapshot,
        toasts: Toasts,
        state: State,
        time: f64,
    }

    impl Harness {
        /// N4N0's light on, fast walk off.
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let recorder = Arc::new(Recorder::default());
            let ctl: Ctl = recorder.clone();
            let mut snap = Snapshot::new(Settings::default());
            snap.n4n0.light = Some(N4n0Light { on: true, color: 1, level: 0.6 });
            Self { ctx, recorder, ctl, snap, toasts: Toasts::default(), state: State::default(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.frame_at(self.time + 1.0 / 60.0, events);
        }

        /// A frame at `time`; how soon it asked for the next.
        fn frame_at(&mut self, time: f64, events: Vec<Event>) -> Duration {
            self.time = time;
            let input =
                RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1048.0, 780.0))), time: Some(self.time), events, ..Default::default() };
            let (ctl, snap, toasts, state) = (&self.ctl, &self.snap, &mut self.toasts, &mut self.state);
            let mut output = self.ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl, snap, toasts: &mut *toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                show(ui, &mut cx, state);
            });
            // Nothing paints these frames.
            output.textures_delta.clear();
            crate::pages::rest::asked(&output)
        }

        /// Frames until what entered stands still.
        fn settle(&mut self) {
            for _ in 0..90 {
                self.frame(Vec::new());
            }
        }

        fn press(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at), button(at, true)]);
        }

        fn release(&mut self, at: Pos2) {
            self.frame(vec![button(at, false)]);
        }

        fn page(&self) -> Page {
            self.state.host.as_ref().map_or(Page::Home, |host| host.menu.page)
        }

        fn menu(&self) -> &Menu {
            &self.state.host.as_ref().expect("Home shown").menu
        }
    }

    fn button(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// The middle of a feature's card on All tools at the default window: four to a row, wherever the features this
    /// computer has put it.
    fn card_of(target: Target) -> Pos2 {
        let at = features().position(|t| t.0 == target).expect("a feature here");
        pos2(147.0 + 250.0 * (at % 4) as f32, 204.0 + 144.0 * (at / 4) as f32)
    }

    /// Home drew itself twice a second to look at the music, and YouTube's page ten times a second to look for
    /// answers, whether anything had changed or not (the architecture review's DUI-27): the window's landing page
    /// never rested. What changes now asks for its own frame.
    #[test]
    fn home_at_rest_asks_for_no_frame() {
        let mut h = Harness::new();
        assert_eq!(crate::pages::rest::frames_a_minute(0.0, |t| h.frame_at(t, Vec::new())), 0, "Home, untouched");
        h.state.open_all_tools();
        let from = h.time;
        assert_eq!(crate::pages::rest::frames_a_minute(from, |t| h.frame_at(t, Vec::new())), 0, "All tools, untouched");
        // YouTube's page, nothing typed: at most its field's caret blinking (it drew 600 frames a minute).
        let youtube = card_of(Target::YouTube);
        h.press(youtube);
        h.release(youtube);
        assert_eq!(h.page(), Page::Search);
        let from = h.time;
        let frames = crate::pages::rest::frames_a_minute(from, |t| h.frame_at(t, Vec::new()));
        println!("YouTube's page at rest: {frames} frames a minute");
        assert!(frames <= 300, "YouTube's page, untouched: {frames} frames a minute");
    }

    /// Home's card names what plays and no more: a song going on is no change there, and is one on the music page.
    #[test]
    fn a_song_going_on_changes_the_music_page_only() {
        let playing = |position| orbiters_n4n0::media::Reading {
            media: Some(Media { title: "Song".into(), position: Some(position), duration: Some(200), ..Default::default() }),
            volume: Some(Volume { percent: 40, muted: false }),
            art: None,
        };
        assert!(music_shown(&playing(10), false) == music_shown(&playing(11), false), "Home");
        assert!(music_shown(&playing(10), true) != music_shown(&playing(11), true), "the music page");
        let mut louder = playing(10);
        louder.volume = Some(Volume { percent: 45, muted: false });
        assert!(music_shown(&playing(10), false) != music_shown(&louder, false));
    }

    /// REQUEST21 6: Home keeps Call, Suggest and Tracking, side by side in one row (compact beside the launcher, cards
    /// alone); every other feature is under All tools, one press
    /// away, and still opens its page there; All tools' Home (or Escape) goes back.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn home_keeps_call_suggest_and_tracking_and_all_tools_has_the_rest() {
        widgets::watch_controls();
        let names = |targets: &[Target]| -> Vec<&'static str> { targets.iter().map(|t| features().find(|f| f.0 == *t).map(|f| menu_text(f.2)).expect("a feature")).collect() };
        let kept = names(&[Target::Call, Target::Suggest, Target::Tracking]);
        let moved = names(&[Target::FastWalk, Target::YouTube, Target::Photo, Target::Music, Target::Light, Target::States, Target::Instances, Target::Qr]);
        assert_eq!(home_tiles().map(|t| t.0).collect::<Vec<_>>(), HOME_ONLY, "the three, in the owner's order");
        assert_eq!(features().count(), 11, "All tools keeps every one");
        for launcher in [false, true] {
            let mut h = Harness::new();
            if launcher {
                let mut data = orbiters_core::launcher::LauncherState::default();
                data.possible.play = true;
                h.state.launcher_mut().set_data(Arc::new(data));
            }
            h.settle();
            assert_eq!(h.state.launcher_mut().shown(), launcher);
            let rects: Vec<Rect> = kept.iter().map(|n| widgets::control(&h.ctx, n).unwrap_or_else(|| panic!("{n} on Home")).0).collect();
            // One row, the page's width shared.
            assert!(rects.iter().all(|r| (r.top() - rects[0].top()).abs() < 0.5 && (r.width() - rects[0].width()).abs() < 0.5), "{rects:?}");
            assert!(rects[0].left() < rects[1].left() && rects[1].left() < rects[2].left());
            for n in &moved {
                // (Previous instances is also Where's button, over Play: not a tile of the row.)
                if let Some((r, _)) = widgets::control(&h.ctx, n) {
                    assert!(*n == names(&[Target::Instances])[0] && launcher && r.height() <= 32.5 && r.bottom() < rects[0].top(), "{n} is not on Home (launcher {launcher}): {r:?}");
                }
            }
            // All tools: every feature, and each opens its page as Home's did.
            let (all, on) = widgets::control(&h.ctx, "All tools").expect("All tools on Home");
            assert!(on);
            h.press(all.center());
            h.release(all.center());
            h.settle();
            for n in kept.iter().chain(&moved) {
                assert!(widgets::control(&h.ctx, n).is_some(), "{n} under All tools");
            }
            let (youtube, _) = widgets::control(&h.ctx, moved[1]).expect("YouTube");
            h.press(youtube.center());
            h.release(youtube.center());
            assert_eq!(h.page(), Page::Search);
            h.frame(vec![key(Key::Escape)]);
            h.settle();
            assert_eq!(h.page(), Page::Home);
            assert!(widgets::control(&h.ctx, moved[0]).is_some(), "back on All tools, where it was opened");
            h.frame(vec![key(Key::Escape)]);
            h.settle();
            assert!(widgets::control(&h.ctx, moved[0]).is_none(), "Escape on All tools: Home");
            assert!(widgets::control(&h.ctx, kept[1]).is_some());
        }
    }

    /// REQUEST20 2: Where above Play. Nothing chosen, Play does what it always did; a card chosen (nothing sent by the
    /// choice), Play goes there: an invite accepted on the profile chosen, a new Friends+ instance made by VRChat first
    /// and gone to, a public one named here without a VRChat sign-in. A chosen card that goes hands the choice back to
    /// Home and says so; with VRChat running on the profile chosen, Where stands aside.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn where_chooses_the_place_play_starts_vrchat_in() {
        use crate::pages::launcher::destinations::{Frequent, HistoryView};
        use orbiters_core::launcher::{Action, Game, GameProfile, InviteKind, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        let mut h = Harness::new();
        let data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, .. } = data.source.clone() else { panic!("the sample is signed in") };
        let invite = data.invites.iter().find(|i| i.kind == InviteKind::Invite).cloned().unwrap();
        let invite_world = invite.place.as_ref().unwrap().world_name.clone();
        let world = "wrld_00000000-0000-4000-8000-000000000301";
        let history = HistoryView { frequent: vec![Frequent { world_id: world.into(), name: "Lantern Garden".into(), visits: 5, score: 3.0, last: 0, picture: None }], last: None };
        let stage = |h: &mut Harness, data: &orbiters_core::launcher::LauncherState, signed_in: bool| {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data.clone()));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(history.clone()), Vec::new(), None, signed_in);
        };
        stage(&mut h, &data, true);
        h.settle();
        let click = |h: &mut Harness, label: &str| {
            let (r, on) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            assert!(on, "{label} takes a press");
            h.press(r.center());
            h.release(r.center());
            h.frame(Vec::new());
        };
        let sent = |h: &Harness| h.recorder.launcher.lock().unwrap().clone();
        let mine = |action: Action| Action::As { account: me.clone(), action: Box::new(action) };
        // Nothing chosen: Play is VRChat's own start, on the profile under the buttons.
        click(&mut h, "Play in VR");
        assert_eq!(sent(&h), [mine(Action::PlayOn { mode: LaunchMode::Vr, profile: 0 })]);
        // The invite chosen: nothing goes until Play, then the invite is accepted into its own instance.
        click(&mut h, &invite_world);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(format!("invite:{}", invite.id).as_str()));
        assert_eq!(sent(&h).len(), 1, "a choice sends nothing");
        click(&mut h, "Play in Desktop");
        assert_eq!(sent(&h).last(), Some(&mine(Action::AcceptInviteOn { invite: invite.id.clone(), mode: LaunchMode::Desktop, profile: 0 })));
        // A place visited often: VRChat makes a new Friends+ instance (in VRChat's default region, none played yet),
        // then VRChat starts right in it.
        click(&mut h, "Lantern Garden");
        assert!(h.recorder.asked.lock().unwrap().is_empty(), "a choice asks nothing of VRChat");
        click(&mut h, "Play in VR");
        for _ in 0..400 {
            if sent(&h).len() == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            h.frame(Vec::new());
        }
        assert_eq!(h.recorder.asked.lock().unwrap().as_slice(), [format!("CreateInstance {{ world: {world:?}, access: FriendsPlus, region: Us }}")]);
        let Some(Action::As { account, action }) = sent(&h).last().cloned() else { panic!("bound to the account") };
        assert_eq!(account, me);
        let Action::PlayAt { mode: LaunchMode::Vr, profile: 0, destination } = *action else { panic!("{action:?}") };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (world, MADE));
        // The chosen invite goes (put away, accepted elsewhere): the choice is Home again, and the page says so.
        click(&mut h, &invite_world);
        let mut gone = data.clone();
        gone.invites.retain(|i| i.id != invite.id);
        h.state.launcher_mut().set_data(Arc::new(gone));
        h.frame(Vec::new());
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None);
        assert!(h.state.launcher_mut().places.notice().is_some_and(|n| n.contains(&invite_world)), "{:?}", h.state.launcher_mut().places.notice());
        // No VRChat sign-in: a new instance is a public one named here; nothing is asked of VRChat.
        stage(&mut h, &data, false);
        h.settle();
        click(&mut h, "Lantern Garden");
        click(&mut h, "Play in Desktop");
        let Some(Action::As { action, .. }) = sent(&h).last().cloned() else { panic!() };
        let Action::PlayAt { destination, .. } = *action else { panic!("{action:?}") };
        assert!(crate::controller::vrchat_data::public_instance(&destination.instance_id), "{}", destination.instance_id);
        assert_eq!(h.recorder.asked.lock().unwrap().len(), 1, "VRChat was asked nothing more");
        // VRChat runs on the profile chosen: Where stands aside (the Play part says it runs).
        let mut running = data.clone();
        running.game = Game::Running(LaunchMode::Vr);
        running.profiles = vec![GameProfile { profile: 0, account: Some("Sample Me".into()), running: Some(LaunchMode::Vr), starting: None, assistant: false }];
        stage(&mut h, &running, true);
        h.settle();
        assert!(orbiters_ui::widgets::control(&h.ctx, "Lantern Garden").is_none());
    }

    #[test]
    fn a_mac_shows_no_feature_that_needs_vrchat_on_it() {
        let hidden = [Target::FastWalk, Target::Photo, Target::Tracking, Target::Qr];
        assert_eq!(features().any(|t| hidden.contains(&t.0)), orbiters_core::osc::VRCHAT_RUNS_HERE);
        assert!(features().any(|t| t.0 == Target::States) && features().any(|t| t.0 == Target::Light), "what goes over OSC stays");
    }

    #[test]
    fn a_card_opens_its_page_on_the_press_and_home_comes_back() {
        let mut h = Harness::new();
        h.state.open_all_tools();
        h.settle();
        // YouTube's card: its page on the press, and the search field takes the keys.
        let youtube = card_of(Target::YouTube);
        h.press(youtube);
        assert_eq!(h.page(), Page::Search, "open on the press, before the release");
        h.release(youtube);
        h.settle();
        h.frame(vec![Event::Text("lofi".into())]);
        assert_eq!(h.menu().query, "lofi", "typed into the menu's field");
        h.frame(vec![key(Key::Escape)]);
        assert_eq!(h.page(), Page::Home);
        h.settle();

        // The light's card (away from its switch) opens the light's page; its Home button comes back on the release.
        let light = card_of(Target::Light) + vec2(0.0, 32.0);
        h.press(light);
        h.release(light);
        assert_eq!(h.page(), Page::Light);
        h.settle();
        let back = pos2(60.0, 64.0);
        h.press(back);
        assert_eq!(h.page(), Page::Light, "a button acts on its release");
        h.release(back);
        assert_eq!(h.page(), Page::Home);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_qr_code_scanner_scans_while_its_page_shows() {
        let mut h = Harness::new();
        h.state.open_all_tools();
        h.settle();
        // The third row: suggest, previous instances, the QR code scanner, then what N4N0 says.
        let qr = pos2(640.0, 490.0);
        h.press(qr);
        h.release(qr);
        assert_eq!(h.page(), Page::Qr);
        assert!(matches!(h.recorder.n4n0.lock().unwrap().as_slice(), [N4n0Command::Qr { on: true, .. }]));
        h.settle();
        // Back home: the scanner stops.
        let back = pos2(60.0, 64.0);
        h.press(back);
        h.release(back);
        assert_ne!(h.page(), Page::Qr);
        assert!(matches!(h.recorder.n4n0.lock().unwrap().as_slice(), [N4n0Command::Qr { on: true, .. }, N4n0Command::Qr { on: false, .. }]));
        h.settle();
        // MENU-13: the window leaves Home for another page on the scanner's page: the camera is let go at once (the
        // window's own), and read again once Home shows again, on the same page.
        h.press(qr);
        h.release(qr);
        assert_eq!(h.page(), Page::Qr);
        h.recorder.n4n0.lock().unwrap().clear();
        let ctl = h.ctl.clone();
        h.state.left(&ctl);
        assert!(matches!(h.recorder.n4n0.lock().unwrap().as_slice(), [N4n0Command::Qr { on: false, holder: orbiters_n4n0::MenuHolder::Window }]));
        h.frame(Vec::new());
        assert_eq!(h.page(), Page::Qr);
        assert!(matches!(h.recorder.n4n0.lock().unwrap().as_slice(), [_, N4n0Command::Qr { on: true, holder: orbiters_n4n0::MenuHolder::Window }]));
        h.press(back);
        h.release(back);
        h.settle();
        // Previous instances, beside it, opens its own page.
        h.press(pos2(397.0, 490.0));
        h.release(pos2(397.0, 490.0));
        assert_eq!(h.page(), Page::Instances);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_switch_acts_alone_and_shows_its_change_at_once() {
        let mut h = Harness::new();
        h.state.open_all_tools();
        h.settle();
        // Fast walk's switch: one change, saved as the setting; the card under it does nothing more.
        let walk = pos2(217.0, 180.0);
        h.press(walk);
        assert!(h.menu().fast_walk, "on at once");
        h.release(walk);
        let patches = h.recorder.patches.lock().unwrap().clone();
        assert_eq!(patches.len(), 1, "{patches:?}");
        assert_eq!(patches[0]["walk_boost"]["enabled"], serde_json::json!(true));
        assert_eq!(h.page(), Page::Home);

        // The light's switch: off at once, and it stays off while the service has not said so yet.
        let light = pos2(466.0, 324.0);
        h.press(light);
        h.release(light);
        h.frame(Vec::new());
        assert!(matches!(h.recorder.n4n0.lock().unwrap().as_slice(), [N4n0Command::Light { on: false, .. }]));
        assert!(!h.menu().light.expect("a light").on, "as switched, though the service still says on");
        assert_eq!(h.page(), Page::Home, "its card did not open the light's page");
    }

    /// Something asked on an avatar's states, then Back to the avatars' cards before the service's list said it (or
    /// the avatar gone from the list): its time was only counted on an avatar's page, so on the cards it stayed for
    /// ever, and the frame asked "when its time is up" was asked at every frame: about twenty a second, at rest.
    #[test]
    fn what_was_asked_of_the_states_ends_on_the_cards_too_and_the_page_rests() {
        use crate::state::{AvatarView, StateView, StatesView};
        widgets::watch_controls();
        for avatar_goes in [false, true] {
            let mut h = Harness::new();
            h.state.open_all_tools();
            let states = vec![StateView { id: "s1".into(), name: "Hoodie on".into(), ..Default::default() }];
            let party = StateView { id: "s9".into(), name: "Party".into(), ..Default::default() };
            h.snap.n4n0.states = StatesView {
                avatar_id: Some("avtr_a".into()),
                avatar_name: Some("Rexouium".into()),
                states: states.clone(),
                avatars: vec![
                    AvatarView { id: "avtr_a".into(), name: Some("Rexouium".into()), worn: true, states },
                    AvatarView { id: "avtr_b".into(), name: Some("Ultirex ✨".into()), worn: false, states: vec![party] },
                ],
                ..Default::default()
            };
            h.settle();
            let card = card_of(Target::States);
            h.press(card);
            h.release(card);
            h.settle();
            // The other avatar's states, and its default set: asked, and the service here never says it.
            let (other, _) = widgets::control(&h.ctx, "Ultirex ✨").expect("its card");
            h.press(other.center());
            h.release(other.center());
            h.settle();
            let (default, _) = widgets::control(&h.ctx, "Set as default").expect("drawn");
            h.press(default.center());
            h.release(default.center());
            assert_eq!(h.state.states.waiting(), 1, "asked, and waiting for the service's list");
            if avatar_goes {
                // Its last state deleted elsewhere, say: its card goes, and the cards show.
                h.snap.n4n0.states.avatars.pop();
                h.frame(Vec::new());
            } else {
                let (back, _) = widgets::control(&h.ctx, "States").expect("back to the cards");
                h.press(back.center());
                h.release(back.center());
            }
            // The cards come in (a second and a half: what was asked has four).
            h.settle();
            assert!(widgets::control(&h.ctx, "Rexouium").is_some(), "the cards");
            assert!(widgets::control(&h.ctx, "Set as default").is_none(), "and no avatar's page");
            assert_eq!(h.state.states.waiting(), 1, "still waiting, on the cards");
            // Its time passes on the cards: it is let go, and the page asks for no frame.
            let from = h.time;
            let frames = crate::pages::rest::frames_a_minute(from, |t| h.frame_at(t, Vec::new()));
            assert_eq!(h.state.states.waiting(), 0, "its time is up (the avatar gone: {avatar_goes})");
            assert_eq!(frames, 0, "the cards at rest (the avatar gone: {avatar_goes})");
        }
    }

    #[test]
    fn the_states_page_saves_and_its_default_acts_on_the_press() {
        use crate::state::{AvatarView, StateView, StatesView};
        widgets::watch_controls();
        let mut h = Harness::new();
        h.state.open_all_tools();
        let states =
            vec![StateView { id: "s1".into(), name: "Hoodie on".into(), parameters: 10, pose_parameters: 2, eye_height: Some(1.6), ..Default::default() }];
        let party = StateView { id: "s9".into(), name: "Party".into(), ..Default::default() };
        h.snap.n4n0.states = StatesView {
            avatar_id: Some("avtr_a".into()),
            avatar_name: Some("Rexouium".into()),
            states: states.clone(),
            avatars: vec![
                AvatarView { id: "avtr_a".into(), name: Some("Rexouium".into()), worn: true, states },
                AvatarView { id: "avtr_b".into(), name: Some("Ultirex ✨".into()), worn: false, states: vec![party] },
            ],
            ..Default::default()
        };
        h.settle();
        // The States card: its page on the press, a card per avatar.
        let card = card_of(Target::States);
        h.press(card);
        assert_eq!(h.page(), Page::States, "open on the press");
        h.release(card);
        h.settle();
        assert!(widgets::control(&h.ctx, "Set as default").is_none(), "the avatars' cards first");
        // Another avatar's card: its states, which apply only once it is worn; its default is its own.
        let (other, _) = widgets::control(&h.ctx, "Ultirex ✨").expect("its card");
        h.press(other.center());
        h.release(other.center());
        h.settle();
        assert!(matches!(widgets::control(&h.ctx, "Apply"), Some((_, false))), "applied once it is worn");
        let (default, _) = widgets::control(&h.ctx, "Set as default").expect("drawn");
        h.press(default.center());
        assert!(
            matches!(h.recorder.n4n0.lock().unwrap().last(), Some(N4n0Command::States { action: StateAction::Default { id: Some(id) }, avatar: Some(a) }) if id == "s9" && a == "avtr_b"),
            "{:?}",
            h.recorder.n4n0.lock().unwrap()
        );
        h.release(default.center());
        // Back to the cards, then the avatar worn's.
        let (back, _) = widgets::control(&h.ctx, "States").expect("back to the cards");
        h.press(back.center());
        h.release(back.center());
        h.settle();
        let (worn, _) = widgets::control(&h.ctx, "Rexouium").expect("its card");
        h.press(worn.center());
        h.release(worn.center());
        h.settle();
        // Set as default is a choice: asked on the press, before the release.
        let (default, _) = widgets::control(&h.ctx, "Set as default").expect("drawn");
        h.press(default.center());
        assert!(
            matches!(h.recorder.n4n0.lock().unwrap().last(), Some(N4n0Command::States { action: StateAction::Default { id: Some(id) }, avatar: None }) if id == "s1"),
            "{:?}",
            h.recorder.n4n0.lock().unwrap()
        );
        h.release(default.center());
        h.frame(Vec::new());
        assert!(widgets::control(&h.ctx, "Default").is_some(), "marked at once, before the service says so");
        // Save with no name: the next "State n".
        let (save, _) = widgets::control(&h.ctx, "Save").expect("drawn");
        h.press(save.center());
        h.release(save.center());
        assert!(matches!(
            h.recorder.n4n0.lock().unwrap().last(),
            Some(N4n0Command::States { action: StateAction::Save { name: Some(name), .. }, avatar: None }) if name == "State 2"
        ));
    }

    /// REQUEST21 7: Where's Previous instances. Opening the list sends nothing; a row chosen is asked of VRChat (whether
    /// it is still open), and only an open one becomes Where's choice, Play then starting VRChat in that very instance;
    /// one VRChat says is closed offers a new instance of its world, which Play makes first and starts in; Escape puts the
    /// list away with the choice kept. Signed out of VRChat, the list still shows and a row is a new public instance.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_previous_instance_chosen_in_where_is_where_play_starts_vrchat() {
        use crate::pages::launcher::destinations::HistoryView;
        use crate::pages::launcher::previous::Visit;
        use orbiters_core::launcher::{Action, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        let mut h = Harness::new();
        let data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, .. } = data.source.clone() else { panic!("the sample is signed in") };
        let world = |n: u32| format!("wrld_00000000-0000-4000-8000-{:012}", 500 + n);
        let rows = vec![
            Visit { world_id: world(1), instance: "31337~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)".into(), name: "Harbor Market".into(), at: 1_794_990_000 },
            Visit { world_id: world(2), instance: "4242~friends(usr_00000000-0000-4000-8000-000000000005)~region(jp)".into(), name: "Snowy Cabin".into(), at: 1_794_900_000 },
        ];
        let stage = |h: &mut Harness, data: &orbiters_core::launcher::LauncherState| {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data.clone()));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(HistoryView::default()), Vec::new(), None, true);
            l.places.previous.stand_in_rows(rows.clone());
        };
        stage(&mut h, &data);
        h.settle();
        let click = |h: &mut Harness, label: &str| {
            let (r, on) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            assert!(on, "{label} takes a press");
            h.press(r.center());
            h.release(r.center());
            for _ in 0..30 {
                h.frame(Vec::new());
            }
        };
        let until = |h: &mut Harness, what: &str, done: &dyn Fn(&mut Harness) -> bool| {
            for _ in 0..400 {
                if done(h) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
                h.frame(Vec::new());
            }
            panic!("{what}");
        };
        let sent = |h: &Harness| h.recorder.launcher.lock().unwrap().clone();
        // The list: nothing is sent or asked by opening it.
        click(&mut h, "Previous instances");
        assert!(h.state.launcher_mut().places.previous.is_up());
        assert!(orbiters_ui::widgets::control(&h.ctx, "Harbor Market").is_some() && orbiters_ui::widgets::control(&h.ctx, "Snowy Cabin").is_some());
        assert!(h.recorder.asked.lock().unwrap().is_empty() && sent(&h).is_empty(), "opening the list sends nothing");
        // Open: chosen once VRChat said so; Escape keeps the choice; Play starts VRChat in that instance.
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Harbor Market");
        until(&mut h, "VRChat's answer", &|h| h.state.launcher_mut().places.chosen_key().is_some());
        assert_eq!(h.recorder.asked.lock().unwrap().as_slice(), [format!("InstanceSeen {{ world: {:?}, instance: {:?}, max_age_ms: 30000 }}", world(1), rows[0].instance)]);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(rows[0].key().as_str()));
        assert!(sent(&h).is_empty(), "a choice starts nothing");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        assert!(!h.state.launcher_mut().places.previous.is_up(), "Escape puts the list away");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(rows[0].key().as_str()), "with the choice kept");
        click(&mut h, "Play in VR");
        let Some(Action::As { account, action }) = sent(&h).last().cloned() else { panic!("bound to the account: {:?}", sent(&h)) };
        assert_eq!(account, me);
        let Action::PlayAt { mode: LaunchMode::Vr, profile: 0, destination } = *action else { panic!("{action:?}") };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (world(1).as_str(), rows[0].instance.as_str()));
        // Closed: not chosen by itself; a new instance of its world, made by VRChat at Play, then started in.
        *h.recorder.alive.lock().unwrap() = Some(false);
        click(&mut h, "Previous instances");
        click(&mut h, "Snowy Cabin");
        until(&mut h, "VRChat's answer", &|h| orbiters_ui::widgets::control(&h.ctx, "New instance of this world").is_some());
        assert_ne!(h.state.launcher_mut().places.chosen_key(), Some(rows[1].key().as_str()), "a closed instance is not taken silently");
        click(&mut h, "New instance of this world");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(rows[1].key().as_str()));
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        click(&mut h, "Play in Desktop");
        until(&mut h, "the new instance and the start", &|h| sent(&h).len() == 2);
        assert!(h.recorder.asked.lock().unwrap().iter().any(|a| a == &format!("CreateInstance {{ world: {:?}, access: FriendsPlus, region: Jp }}", world(2))), "{:?}", h.recorder.asked.lock().unwrap());
        let Some(Action::As { action, .. }) = sent(&h).last().cloned() else { panic!() };
        let Action::PlayAt { mode: LaunchMode::Desktop, destination, .. } = *action else { panic!("{action:?}") };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (world(2).as_str(), MADE));
        // Signed out of VRChat: the list shows, a row is a new public instance at once, nothing asked of VRChat.
        let mut out = data.clone();
        out.source = Source::Nothing;
        out.possible = orbiters_core::launcher::Possible { play: true, ..Default::default() };
        out.invites.clear();
        let mut h = Harness::new();
        stage(&mut h, &out);
        h.settle();
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        assert!(h.recorder.asked.lock().unwrap().is_empty(), "nothing asked of VRChat signed out");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(rows[0].key().as_str()));
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        click(&mut h, "Play in VR");
        let last = match sent(&h).last().cloned() {
            Some(Action::As { action, .. }) => *action,
            Some(other) => other,
            None => panic!("Play sent nothing"),
        };
        let Action::PlayAt { destination, .. } = last else { panic!("{last:?}") };
        assert_eq!(destination.world_id, world(1));
        assert!(crate::controller::vrchat_data::public_instance(&destination.instance_id) && destination.instance_id.ends_with("~region(eu)"), "{}", destination.instance_id);
    }

    /// The account Where's previous instances are for, as the VRChat sign-in says (made-up): `user` signed in, or
    /// connecting with its last known name.
    fn vrchat_user(id: &str, name: &str) -> orbiters_core::vrchat_api::User {
        serde_json::from_value(serde_json::json!({ "id": id, "displayName": name })).unwrap()
    }

    /// verify-r21 Major 1: with VRChat's session being restored (Connecting) and the friends list not read yet, the
    /// previous instances' list neither offers to sign in nor turns a pick into a new public instance: the instance
    /// picked is kept, and asked about once VRChat says who is signed in, even before the friends list arrives.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn previous_instances_while_vrchat_connects_keep_the_pick_until_it_can_be_checked() {
        use crate::pages::launcher::destinations::HistoryView;
        use crate::pages::launcher::previous::Visit;
        use orbiters_core::launcher::{Action, Source};
        orbiters_ui::widgets::watch_controls();
        let mut h = Harness::new();
        let mut data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, name } = data.source.clone() else { panic!("the sample is signed in") };
        // The friends list is VRCNext's copy (or nothing) while the session is restored.
        data.source = Source::Nothing;
        let world = "wrld_00000000-0000-4000-8000-000000000601".to_string();
        let instance = "31337~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)".to_string();
        let rows = vec![Visit { world_id: world.clone(), instance: instance.clone(), name: "Harbor Market".into(), at: 1_794_990_000 }];
        {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data.clone()));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(HistoryView::default()), Vec::new(), None, true);
            l.places.previous.stand_in_rows(rows.clone());
        }
        h.snap.vrchat_login.restoring = true;
        h.snap.vrchat_login.known = Some(vrchat_user(&me, &name));
        h.settle();
        let click = |h: &mut Harness, label: &str| {
            let (r, on) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            assert!(on, "{label} takes a press");
            h.press(r.center());
            h.release(r.center());
            for _ in 0..30 {
                h.frame(Vec::new());
            }
        };
        assert!(orbiters_ui::widgets::control(&h.ctx, "Sign in to VRChat").is_none(), "no sign-in while connecting");
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        assert_eq!(h.state.launcher_mut().places.desk.selection().instance(), Some(&rows[0]), "kept, not turned into a new public instance while connecting");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None, "not chosen before it was checked");
        assert!(h.recorder.asked.lock().unwrap().is_empty(), "nothing asked of VRChat before it says who is signed in");
        // VRChat says who it is; the friends list is still being read.
        *h.recorder.alive.lock().unwrap() = Some(true);
        h.snap.vrchat_login.restoring = false;
        h.snap.vrchat_login.allowed = true;
        h.snap.vrchat_login.known = None;
        h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
        for _ in 0..400 {
            if h.state.launcher_mut().places.chosen_key().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            h.frame(Vec::new());
        }
        assert_eq!(h.recorder.asked.lock().unwrap().as_slice(), [format!("InstanceSeen {{ world: {world:?}, instance: {instance:?}, max_age_ms: 30000 }}")]);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(rows[0].key().as_str()), "rejoined once checked");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        click(&mut h, "Play in VR");
        let last = match h.recorder.launcher.lock().unwrap().last().cloned() {
            Some(Action::As { action, .. }) => *action,
            Some(other) => other,
            None => panic!("Play sent nothing"),
        };
        let Action::PlayAt { destination, .. } = last else { panic!("{last:?}") };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (world.as_str(), instance.as_str()));
    }

    /// verify-r21 Major 4: answers about instances cross. A later pick is never lost to an earlier pick's answer that
    /// arrived after it, and a pick the person left (another card chosen) is not chosen again by its late answer.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_late_answer_never_takes_back_a_later_choice() {
        use crate::pages::launcher::destinations::HistoryView;
        use crate::pages::launcher::previous::Visit;
        use orbiters_core::launcher::Source;
        orbiters_ui::widgets::watch_controls();
        let mut h = Harness::new();
        let data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, name } = data.source.clone() else { panic!("the sample is signed in") };
        let world = |n: u32| format!("wrld_00000000-0000-4000-8000-{:012}", 700 + n);
        let a = Visit { world_id: world(1), instance: "111~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)".into(), name: "Harbor Market".into(), at: 1_794_990_000 };
        let b = Visit { world_id: world(2), instance: "222~friends(usr_00000000-0000-4000-8000-000000000005)~region(jp)".into(), name: "Snowy Cabin".into(), at: 1_794_900_000 };
        {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data.clone()));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(HistoryView::default()), Vec::new(), None, true);
            l.places.previous.stand_in_rows(vec![a.clone(), b.clone()]);
        }
        h.snap.vrchat_login.allowed = true;
        h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
        h.settle();
        let press = |h: &mut Harness, label: &str| {
            let (r, _) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            h.press(r.center());
            h.release(r.center());
        };
        // A answers after B, both before the next frame: B, the later pick, is what is chosen.
        h.recorder.alive_by.lock().unwrap().insert(a.instance.clone(), (Some(true), Duration::from_millis(150)));
        h.recorder.alive_by.lock().unwrap().insert(b.instance.clone(), (Some(true), Duration::from_millis(40)));
        press(&mut h, "Previous instances");
        h.settle();
        press(&mut h, "Harbor Market");
        press(&mut h, "Snowy Cabin");
        std::thread::sleep(Duration::from_millis(300));
        for _ in 0..30 {
            h.frame(Vec::new());
        }
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(b.key().as_str()), "the later pick, not left asking");
        // A picked, then left for Home world before VRChat answers: its late answer chooses nothing.
        h.recorder.alive_by.lock().unwrap().insert(a.instance.clone(), (Some(true), Duration::from_millis(200)));
        press(&mut h, "Harbor Market");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        press(&mut h, "Home world");
        std::thread::sleep(Duration::from_millis(350));
        for _ in 0..30 {
            h.frame(Vec::new());
        }
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None, "Home world stays chosen");
    }

    /// Where, signed in to VRChat (verified) with the sample's friends list, its previous instances `rows` handed in.
    fn where_signed_in(rows: Vec<crate::pages::launcher::previous::Visit>) -> Harness {
        use orbiters_core::launcher::Source;
        let mut h = Harness::new();
        let data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, name } = data.source.clone() else { panic!("the sample is signed in") };
        {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(crate::pages::launcher::destinations::HistoryView::default()), Vec::new(), None, true);
            if !rows.is_empty() {
                l.places.previous.stand_in_rows(rows);
            }
        }
        h.snap.vrchat_login.allowed = true;
        h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
        h.settle();
        h
    }

    fn click(h: &mut Harness, label: &str) {
        let (r, on) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
        assert!(on, "{label} takes a press");
        h.press(r.center());
        h.release(r.center());
        for _ in 0..30 {
            h.frame(Vec::new());
        }
    }

    /// A Mac: the profile choice of Windows' Home under its one Play button, for one VRChat at a time. Play starts the profile
    /// chosen; while one runs or starts, no other does, and nothing is sent.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_s_play_starts_the_profile_chosen_and_none_beside_a_running_one() {
        use orbiters_core::launcher::{Action, Game, GameProfile, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        let mut h = where_signed_in(Vec::new());
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        let Source::SignedIn { user_id: me, name } = data.source.clone() else { panic!("the sample is signed in") };
        data.invites.clear();
        data.game = Game::NotRunning;
        let stage = |h: &mut Harness, data: &mut orbiters_core::launcher::LauncherState, game: Game, on: Option<u32>| {
            let mode = |p: u32, up: bool| (up && on == Some(p)).then_some(LaunchMode::Desktop);
            let (running, starting) = (matches!(game, Game::Running(_)), matches!(game, Game::Starting(_)));
            let profile = |p: u32, account: &str| GameProfile { profile: p, account: Some(account.into()), running: mode(p, running), starting: mode(p, starting), assistant: false };
            data.profiles = vec![profile(0, &name), profile(1, "Sample Alt")];
            data.game = game;
            h.state.launcher_mut().set_data(Arc::new(data.clone()));
            h.settle();
        };
        stage(&mut h, &mut data, Game::NotRunning, None);
        let sent = |h: &Harness| h.recorder.launcher.lock().unwrap().iter().filter(|a| matches!(a, Action::As { action, .. } if matches!(**action, Action::PlayOn { .. }))).cloned().collect::<Vec<_>>();
        let on = |profile| Action::As { account: me.clone(), action: Box::new(Action::PlayOn { mode: LaunchMode::Desktop, profile }) };
        let own = format!("0 · {name}");
        click(&mut h, "Play VRChat");
        assert_eq!(sent(&h), [on(0)], "the default profile");
        click(&mut h, "1 · Sample Alt");
        click(&mut h, "Play VRChat");
        assert_eq!(sent(&h), [on(0), on(1)]);
        click(&mut h, "Another profile");
        click(&mut h, "Play VRChat");
        assert_eq!(sent(&h), [on(0), on(1), on(2)]);
        let played = |h: &Harness| orbiters_ui::widgets::control(&h.ctx, "Play VRChat").expect("the button stays").1;
        for game in [Game::Running(LaunchMode::Desktop), Game::Starting(LaunchMode::Desktop)] {
            stage(&mut h, &mut data, game, Some(1));
            for chip in ["Profile 2", own.as_str(), "1 · Sample Alt"] {
                click(&mut h, chip);
                assert!(!played(&h), "{chip}: Play starts nothing");
                let (r, _) = orbiters_ui::widgets::control(&h.ctx, "Play VRChat").unwrap();
                h.press(r.center());
                h.release(r.center());
            }
            assert_eq!(sent(&h).len(), 3);
        }
        // Quit: the one chosen starts.
        stage(&mut h, &mut data, Game::NotRunning, None);
        click(&mut h, &own);
        click(&mut h, "Play VRChat");
        assert_eq!(sent(&h).last(), Some(&on(0)));
        assert!(orbiters_ui::widgets::control(&h.ctx, "Play in VR").is_none() && orbiters_ui::widgets::control(&h.ctx, "Play in Desktop").is_none());
    }

    /// K2: where several profiles are used, an invite is offered on any profile not known to be another's (VRChat starts without it,
    /// and it is delivered once its account is proven), and an event's private place too; on one known to be another's, as
    /// before, not the invite. While VRChat's log cannot be read, nothing private is, and Home says that this is why and what to
    /// do.
    #[test]
    fn a_private_place_is_offered_on_any_profile_not_known_to_be_another_s() {
        use crate::pages::launcher::destinations::EventSoon;
        use orbiters_core::launcher::{Action, Game, GameProfile, InstanceAccess, Invite, InviteKind, LogTrouble, Place, Source};
        orbiters_ui::widgets::watch_controls();
        let play = if cfg!(target_os = "macos") { "Play VRChat" } else { "Play in Desktop" };
        let mut h = where_signed_in(Vec::new());
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        let Source::SignedIn { name, .. } = data.source.clone() else { panic!("the sample is signed in") };
        data.game = Game::NotRunning;
        data.several_profiles = true;
        data.profiles = vec![
            GameProfile { profile: 0, account: Some(name.clone()), ..Default::default() },
            // Nobody was ever seen on it: whose it is, the VRChat started there will say.
            GameProfile { profile: 1, ..Default::default() },
            GameProfile { profile: 2, account: Some("Sample Alt".into()), ..Default::default() },
        ];
        data.invites = vec![Invite {
            id: "synthetic".into(),
            from_id: String::new(),
            from_name: "Sample".into(),
            kind: InviteKind::Invite,
            from_icon: String::new(),
            received_at: 0,
            message: String::new(),
            place: Some(Place {
                world_id: "wrld_00000000-0000-4000-8000-000000000801".into(),
                instance_id: "7~private(test)~region(eu)~nonce(synthetic)".into(),
                world_name: "Sample private room".into(),
                world_picture: String::new(),
                access: InstanceAccess::Invite,
                region: "eu".into(),
            }),
        }];
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        let event = |id: &str, n: u32, title: &str, instance: &str| EventSoon { id: id.into(), title: title.into(), starts_at: 1_795_000_000, location: Some(format!("wrld_00000000-0000-4000-8000-00000000090{n}:{instance}")), ..Default::default() };
        let events = vec![event("a", 1, "Sample open meetup", "5~region(eu)"), event("b", 2, "Sample closed meetup", "6~group(grp_x)~groupAccessType(members)~region(eu)")];
        h.state.launcher_mut().places.stand_in(Some(crate::pages::launcher::destinations::HistoryView::default()), events, None, true);
        h.settle();
        let sent = |h: &mut Harness| -> Vec<Action> {
            h.recorder.launcher.lock().unwrap().drain(..).map(|a| if let Action::As { action, .. } = a { *action } else { a }).collect()
        };
        let there = |h: &Harness, label: &str| orbiters_ui::widgets::control(&h.ctx, label).is_some();
        let own = format!("0 · {name}");
        for (chip, profile) in [(own.as_str(), 0), ("Profile 1", 1), ("Another profile", 3)] {
            click(&mut h, chip);
            click(&mut h, "Sample private room");
            click(&mut h, play);
            assert!(matches!(sent(&mut h).as_slice(), [Action::AcceptInviteOn { invite, profile: p, .. }] if invite == "synthetic" && *p == profile), "{chip}");
            click(&mut h, "Sample open meetup");
            click(&mut h, play);
            assert!(matches!(sent(&mut h).as_slice(), [Action::PlayAt { destination, profile: p, .. }] if destination.instance_id == "5~region(eu)" && *p == profile), "{chip}");
            click(&mut h, "Home world");
        }
        // (Without the invite, its card fits.)
        let invites = std::mem::take(&mut data.invites);
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        click(&mut h, "Profile 1");
        click(&mut h, "Sample closed meetup");
        click(&mut h, play);
        assert!(matches!(sent(&mut h).as_slice(), [Action::PlayAt { destination, profile: 1, .. }] if destination.instance_id.starts_with("6~group")));
        click(&mut h, "Home world");
        data.invites = invites;
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        // Another account's: nothing private (an event's invite arrives at home).
        click(&mut h, "2 · Sample Alt");
        assert!(!there(&h, "Sample private room"));
        click(&mut h, "Sample closed meetup");
        click(&mut h, play);
        assert!(matches!(sent(&mut h).as_slice(), [Action::PlayOn { profile: 2, .. }]));
        // VRChat's log cannot be read: nothing private, on any profile; a public place and VRChat's own start stay.
        click(&mut h, &own);
        for trouble in [LogTrouble::NoFolder, LogTrouble::NotWritten, LogTrouble::Denied("/x/VRChat".into())] {
            data.log_trouble = Some(trouble.clone());
            h.state.launcher_mut().set_data(Arc::new(data.clone()));
            h.settle();
            assert!(!there(&h, "Sample private room"), "{trouble:?}");
            click(&mut h, "Sample open meetup");
            click(&mut h, play);
            assert!(matches!(sent(&mut h).as_slice(), [Action::PlayAt { destination, .. }] if destination.instance_id == "5~region(eu)"), "{trouble:?}");
            click(&mut h, "Home world");
            click(&mut h, play);
            assert!(matches!(sent(&mut h).as_slice(), [Action::PlayOn { profile: 0, .. }]), "{trouble:?}");
            let said = crate::pages::launcher::destinations::log_words(&trouble);
            assert!(said.contains("Private places need VRChat's log") && (said.contains("turn on Logging") || said.contains("allow Orbiters - N4N0 to read")), "why, and what to do: {said}");
        }
        // One profile alone: as it always was, whatever its log.
        data.several_profiles = false;
        data.profiles = vec![GameProfile::default()];
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        click(&mut h, "Sample private room");
        click(&mut h, play);
        assert!(matches!(sent(&mut h).as_slice(), [Action::AcceptInviteOn { profile: 0, .. }]));
    }

    /// R1: while a private place is on its way Home says both steps, then why it was not delivered, with another try; × gives it
    /// up. Nothing of it is said where no place is.
    #[test]
    fn home_says_both_steps_of_a_private_place_and_why_it_was_not_delivered() {
        use crate::pages::launcher::hero::joining_line;
        use orbiters_core::launcher::{Action, Game, JoinFailure, JoinStep, Joining, LaunchMode, LogTrouble};
        orbiters_ui::widgets::watch_controls();
        let joining = |step: JoinStep| Joining { world: "Sample private room".into(), step };
        assert!(joining_line(&joining(JoinStep::Starting)).starts_with("Starting VRChat, then joining Sample private room"));
        assert!(joining_line(&joining(JoinStep::SigningIn)).contains("who is signed in"));
        for (why, says) in [
            (JoinFailure::OtherAccount, "another account"),
            (JoinFailure::NoProof, "did not say in time"),
            (JoinFailure::Gone, "closed too soon"),
            (JoinFailure::Log(LogTrouble::NotWritten), "Logging"),
            (JoinFailure::NotSent("no network".into()), "no network"),
        ] {
            let line = joining_line(&joining(JoinStep::Failed(why)));
            assert!(line.starts_with("Sample private room was not joined") && line.contains(says), "{line}");
        }
        let mut h = where_signed_in(Vec::new());
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        let there = |h: &Harness, label: &str| orbiters_ui::widgets::control(&h.ctx, label).is_some();
        let again = |h: &Harness| h.recorder.launcher.lock().unwrap().iter().map(|a| if let Action::As { action, .. } = a { (**action).clone() } else { a.clone() }).filter(|a| matches!(a, Action::JoinAgain | Action::ForgetJoining)).collect::<Vec<_>>();
        data.game = Game::Starting(LaunchMode::Desktop);
        data.joining = Some(joining(JoinStep::Starting));
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        assert!(!there(&h, "Try again"), "on its way");
        data.game = Game::Running(LaunchMode::Desktop);
        data.joining = Some(joining(JoinStep::Failed(JoinFailure::OtherAccount)));
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        click(&mut h, "Try again");
        assert_eq!(again(&h), [Action::JoinAgain]);
        data.joining = None;
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        assert!(!there(&h, "Try again"));
    }

    /// Tenth verification: Join into a game that runs asks for that game alone (none is started for it if it is gone by then):
    /// on an invite's card, and on a friend's profile.
    #[cfg(target_os = "macos")]
    #[test]
    fn join_into_a_running_game_never_asks_for_a_start() {
        use orbiters_core::launcher::{Action, Game, GameProfile, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        for starting in [false, true] {
            let mut h = where_signed_in(Vec::new());
            let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
            let Source::SignedIn { name, .. } = data.source.clone() else { panic!() };
            let (running, start) = ((!starting).then_some(LaunchMode::Desktop), starting.then_some(LaunchMode::Desktop));
            data.game = if starting { Game::Starting(LaunchMode::Desktop) } else { Game::Running(LaunchMode::Desktop) };
            data.profiles = vec![GameProfile { profile: 0, account: Some(name), running, starting: start, assistant: false }, GameProfile { profile: 2, ..Default::default() }];
            h.state.launcher_mut().set_data(Arc::new(data));
            h.settle();
            let sent = |h: &Harness| h.recorder.launcher.lock().unwrap().iter().map(|a| if let Action::As { action, .. } = a { (**action).clone() } else { a.clone() }).collect::<Vec<_>>();
            if !starting {
                click(&mut h, "Join now");
                assert!(sent(&h).iter().any(|a| matches!(a, Action::AcceptInviteRunning { .. })) && !sent(&h).iter().any(|a| matches!(a, Action::AcceptInvite { .. })), "{:?}", sent(&h));
            }
            open_profile_of(&mut h, "Nova Sample");
            click(&mut h, "Join");
            assert!(sent(&h).iter().any(|a| matches!(a, Action::JoinRunning { .. })) && !sent(&h).iter().any(|a| matches!(a, Action::Play { .. } | Action::PlayAt { .. })), "{:?}", sent(&h));
        }
    }

    /// R4: the × of the chip chosen takes a profile that does not run off Home's choice (the default one's, one VRChat runs on, and
    /// one not chosen have none), also one only added here.
    #[test]
    fn a_profile_s_chip_can_be_taken_off_the_choice() {
        use orbiters_core::launcher::{Action, Game, GameProfile, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        let mut h = where_signed_in(Vec::new());
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        let Source::SignedIn { name, .. } = data.source.clone() else { panic!("the sample is signed in") };
        data.game = Game::NotRunning;
        data.profiles = vec![GameProfile { profile: 0, account: Some(name.clone()), ..Default::default() }, GameProfile { profile: 1, account: Some("Sample Alt".into()), ..Default::default() }];
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        let x_of = |h: &mut Harness, label: &str| {
            let (chip, _) = orbiters_ui::widgets::control(&h.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            h.press(pos2(chip.right() - 14.0, chip.center().y));
            h.release(pos2(chip.right() - 14.0, chip.center().y));
            h.settle();
        };
        let forgot = |h: &mut Harness| -> Vec<u32> {
            let sent: Vec<Action> = h.recorder.launcher.lock().unwrap().drain(..).map(|a| if let Action::As { action, .. } = a { *action } else { a }).collect();
            sent.into_iter().filter_map(|a| if let Action::ForgetProfile { profile } = a { Some(profile) } else { None }).collect()
        };
        let own = format!("0 · {name}");
        x_of(&mut h, &own);
        x_of(&mut h, &own);
        assert_eq!(forgot(&mut h), [0u32; 0], "the default one stays");
        // The first press chooses it; its × then takes it off.
        x_of(&mut h, "1 · Sample Alt");
        assert_eq!(forgot(&mut h), [0u32; 0]);
        x_of(&mut h, "1 · Sample Alt");
        assert_eq!(forgot(&mut h), [1]);
        // VRChat runs on it: no ×.
        data.game = Game::Running(LaunchMode::Desktop);
        data.profiles[1].running = Some(LaunchMode::Desktop);
        h.state.launcher_mut().set_data(Arc::new(data.clone()));
        h.settle();
        x_of(&mut h, "1 · Sample Alt");
        x_of(&mut h, "1 · Sample Alt");
        assert_eq!(forgot(&mut h), [0u32; 0]);
        // One added here and never started.
        data.game = Game::NotRunning;
        data.profiles.truncate(1);
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        click(&mut h, "Another profile");
        x_of(&mut h, "Profile 1");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Profile 1").is_none(), "gone at once");
    }

    fn frames_until(h: &mut Harness, what: &str, done: impl Fn(&mut Harness) -> bool) {
        for _ in 0..400 {
            if done(h) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
            h.frame(Vec::new());
        }
        panic!("{what}");
    }

    /// verify-r21 Major 4: VRChat's "open" is good for a minute (`previous::OPEN_FOR`). Play on an instance chosen
    /// longer ago asks VRChat again first: still open, VRChat starts there; closed since, nothing starts and the page
    /// says so.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn an_old_open_is_asked_again_at_play_and_a_closed_one_starts_nothing() {
        use crate::pages::launcher::previous::{Visit, OPEN_FOR};
        use orbiters_core::launcher::Action;
        orbiters_ui::widgets::watch_controls();
        let visit = Visit { world_id: "wrld_00000000-0000-4000-8000-000000000801".into(), instance: "4242~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)".into(), name: "Harbor Market".into(), at: 1_794_990_000 };
        let mut h = where_signed_in(vec![visit.clone()]);
        let instances = |h: &Harness| h.recorder.asked.lock().unwrap().iter().filter(|a| a.starts_with("Instance")).count();
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        // Within the minute: Play goes at once, nothing asked again.
        click(&mut h, "Play in VR");
        assert_eq!(instances(&h), 1);
        assert_eq!(h.recorder.launcher.lock().unwrap().len(), 1);
        // Older than that, closed meanwhile: asked again, nothing starts, the choice goes, and the page says why.
        h.state.launcher_mut().places.desk.age_check(OPEN_FOR + Duration::from_secs(5));
        *h.recorder.alive.lock().unwrap() = Some(false);
        click(&mut h, "Play in VR");
        frames_until(&mut h, "asked again", |h| h.recorder.asked.lock().unwrap().iter().filter(|a| a.starts_with("Instance")).count() == 2);
        frames_until(&mut h, "answered", |h| !h.state.launcher_mut().places.desk.rechecking());
        assert_eq!(h.recorder.launcher.lock().unwrap().len(), 1, "a closed instance starts nothing");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None);
        assert!(h.state.launcher_mut().places.notice().is_some_and(|n| n.contains("Harbor Market")), "{:?}", h.state.launcher_mut().places.notice());
        // Chosen again, aged, still open: asked again, then VRChat starts there.
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        h.state.launcher_mut().places.desk.age_check(OPEN_FOR + Duration::from_secs(5));
        let asked = instances(&h);
        click(&mut h, "Play in VR");
        frames_until(&mut h, "started after asking", |h| h.recorder.launcher.lock().unwrap().len() == 2);
        assert_eq!(instances(&h), asked + 1);
        let last = match h.recorder.launcher.lock().unwrap().last().cloned() {
            Some(Action::As { action, .. }) => *action,
            Some(other) => other,
            None => unreachable!(),
        };
        let Action::PlayAt { destination, .. } = last else { panic!("{last:?}") };
        assert_eq!(destination.instance_id, visit.instance);
    }

    /// verify-r21 Major 5: the history read is slow; the sheet is closed before it ends and opened again after: the
    /// rows come (it never stays on its stand-ins).
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_history_read_ending_while_the_sheet_is_closed_is_not_lost() {
        use crate::pages::launcher::previous::Visit;
        orbiters_ui::widgets::watch_controls();
        let visit = Visit { world_id: "wrld_00000000-0000-4000-8000-000000000851".into(), instance: "77~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)".into(), name: "Snowy Cabin".into(), at: 1_794_990_000 };
        let mut h = where_signed_in(Vec::new());
        let (permit, wait) = std::sync::mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (counted, rows) = (reads.clone(), vec![visit.clone()]);
        h.state.launcher_mut().places.previous.stand_in_reader(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
            rows.clone()
        });
        click(&mut h, "Previous instances");
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1, "read once the sheet opens");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        assert!(!h.state.launcher_mut().places.previous.is_up());
        // The read ends while the sheet is closed.
        permit.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        click(&mut h, "Previous instances");
        let _ = permit.send(());
        frames_until(&mut h, "the rows after reopening", |h| orbiters_ui::widgets::control(&h.ctx, "Snowy Cabin").is_some());
    }

    /// verify-r21 Major 6: opening the previous instances from an empty cache sends nothing about them (no world's page,
    /// no picture): their pictures are those kept on this computer or none. The first thing that goes is VRChat's "is it
    /// still open?" for the one chosen.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn opening_previous_instances_sends_no_world_id_until_one_is_asked_about() {
        use crate::pages::launcher::previous::Visit;
        orbiters_ui::widgets::watch_controls();
        let rows: Vec<Visit> = (0..8)
            .map(|n| Visit { world_id: format!("wrld_00000000-0000-4000-8000-0000000009{n:02}"), instance: format!("{n}~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)"), name: format!("Quiet World {n}"), at: 1_794_990_000 - n })
            .collect();
        let ids: Vec<String> = rows.iter().map(|r| r.world_id.clone()).collect();
        let mut h = where_signed_in(Vec::new());
        let handed = rows.clone();
        h.state.launcher_mut().places.previous.stand_in_reader(move || handed.clone());
        let sent = || crate::worlds::OUTBOUND.lock().unwrap().iter().filter(|id| ids.contains(id)).count();
        click(&mut h, "Previous instances");
        frames_until(&mut h, "the rows", |h| orbiters_ui::widgets::control(&h.ctx, "Quiet World 0").is_some());
        for _ in 0..60 {
            h.frame(Vec::new());
        }
        assert_eq!(sent(), 0, "no world id sent by opening the list");
        assert!(h.recorder.asked.lock().unwrap().is_empty());
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Quiet World 0");
        frames_until(&mut h, "asked", |h| !h.recorder.asked.lock().unwrap().is_empty());
        assert!(h.recorder.asked.lock().unwrap()[0].starts_with("Instance"), "{:?}", h.recorder.asked.lock().unwrap());
        assert_eq!(sent(), 0, "still nothing to Orbiters about these worlds");
    }

    // --- verify2-r21 M3 and M4: Where's destination is one value with one owner. ---

    /// A row of the history (made-up), `n` its number, of a kind the account can rejoin by its link.
    #[cfg(not(target_os = "macos"))]
    fn row_of(n: u32, name: &str) -> crate::pages::launcher::previous::Visit {
        crate::pages::launcher::previous::Visit {
            world_id: format!("wrld_00000000-0000-4000-8000-{:012}", 860 + n),
            instance: format!("{}~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)", 4240 + n),
            name: name.into(),
            at: 1_794_990_000,
        }
    }

    /// What was sent to start VRChat, unwrapped of the account it is bound to.
    #[cfg(not(target_os = "macos"))]
    fn starts(h: &Harness) -> Vec<orbiters_core::launcher::Action> {
        use orbiters_core::launcher::Action;
        let bare = |a: Action| match a {
            Action::As { action, .. } => *action,
            a => a,
        };
        let start = |a: &Action| matches!(a, Action::Play { .. } | Action::PlayOn { .. } | Action::PlayAt { .. } | Action::AcceptInvite { .. } | Action::AcceptInviteOn { .. });
        h.recorder.launcher.lock().unwrap().iter().cloned().map(bare).filter(start).collect()
    }

    #[cfg(not(target_os = "macos"))]
    fn instance_checks(h: &Harness) -> usize {
        h.recorder.asked.lock().unwrap().iter().filter(|a| a.starts_with("Instance")).count()
    }

    /// Frames and time, for what comes late to come.
    #[cfg(not(target_os = "macos"))]
    fn let_pass(h: &mut Harness, ms: u64) {
        let until = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
            h.frame(Vec::new());
        }
    }

    /// Presses `label` when it takes a press; whether it did.
    #[cfg(not(target_os = "macos"))]
    fn try_click(h: &mut Harness, label: &str) -> bool {
        match orbiters_ui::widgets::control(&h.ctx, label) {
            Some((_, true)) => {
                click(h, label);
                true
            }
            _ => false,
        }
    }

    /// `row_of(1)` chosen and open, its "open" older than a minute, the sheet put away.
    #[cfg(not(target_os = "macos"))]
    fn chosen_and_aged(rows: Vec<crate::pages::launcher::previous::Visit>) -> Harness {
        use crate::pages::launcher::previous::OPEN_FOR;
        orbiters_ui::widgets::watch_controls();
        let first = rows[0].name.clone();
        let mut h = where_signed_in(rows);
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Previous instances");
        click(&mut h, &first);
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        h.state.launcher_mut().places.desk.age_check(OPEN_FOR + Duration::from_secs(5));
        h
    }

    /// verify2-r21 M4, the verifier's own fixture: a pick made while Play's recheck is under way leaves Play usable.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_play_recovers_after_replacing_pick() {
        let visit = row_of(1, "Harbor Market");
        let mut h = chosen_and_aged(vec![visit.clone()]);
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(550)));
        click(&mut h, "Play in VR");
        assert!(h.state.launcher_mut().places.desk.rechecking());
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "replacement chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        assert!(instance_checks(&h) >= 3);
        assert!(h.state.launcher_mut().places.making().is_none(), "replacement resolved but Play is permanently busy");
        // And Play goes where the choice is, once.
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        click(&mut h, "Play in VR");
        frames_until(&mut h, "a start", |h| !starts(h).is_empty());
        let_pass(&mut h, 700);
        assert_eq!(starts(&h).len(), 1, "{:?}", starts(&h));
    }

    /// verify2-r21 M4: the earlier pick's worker, past its own check of the generation, stands still while the later
    /// pick's answer arrives, then finishes: the later answer is neither lost nor replaced.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_a_stale_worker_never_erases_the_current_reply() {
        use crate::pages::launcher::previous::PAUSE;
        orbiters_ui::widgets::watch_controls();
        let (a, b) = (row_of(11, "Harbor Market"), row_of(12, "Snowy Cabin"));
        let mut h = where_signed_in(vec![a.clone(), b.clone()]);
        *h.recorder.alive.lock().unwrap() = Some(true);
        let (go, held) = std::sync::mpsc::channel::<()>();
        *PAUSE.lock().unwrap() = Some((a.instance.clone(), held));
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        std::thread::sleep(Duration::from_millis(150));
        // No frame between B's answer and A's: both wait for the window.
        let (r, _) = orbiters_ui::widgets::control(&h.ctx, "Snowy Cabin").unwrap();
        h.press(r.center());
        std::thread::sleep(Duration::from_millis(250));
        let _ = go.send(());
        std::thread::sleep(Duration::from_millis(150));
        h.release(r.center());
        let_pass(&mut h, 400);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(b.key().as_str()), "the later pick's answer");
    }

    /// verify2-r21 M3: the sheet put away while VRChat connects, with a pick held: Play starts nothing else (not VRChat's
    /// own start, not a card chosen before).
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_play_after_dismissing_the_sheet_while_connecting_starts_nothing_else() {
        use orbiters_core::launcher::{Action, Source};
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(21, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        let Source::SignedIn { user_id: me, name } = h.state.launcher_mut().data.source.clone() else { panic!() };
        let mut data = (*h.state.launcher_mut().data).clone();
        data.source = Source::Nothing;
        h.state.launcher_mut().set_data(Arc::new(data));
        h.snap.vrchat_login.user = None;
        h.snap.vrchat_login.allowed = false;
        h.snap.vrchat_login.restoring = true;
        h.snap.vrchat_login.known = Some(vrchat_user(&me, &name));
        h.settle();
        click(&mut h, "Previous instances");
        // The sheet says it is in front (chat and its keys stand aside), until it is put away.
        assert!(orbiters_ui::front::held(&h.ctx));
        click(&mut h, "Harbor Market");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        assert!(!orbiters_ui::front::held(&h.ctx));
        try_click(&mut h, "Play in VR");
        let_pass(&mut h, 100);
        assert!(starts(&h).is_empty(), "Play while the pick is held: {:?}", starts(&h));
        // VRChat says who it is: the pick is checked, and Play goes there.
        *h.recorder.alive.lock().unwrap() = Some(true);
        h.snap.vrchat_login.restoring = false;
        h.snap.vrchat_login.known = None;
        h.snap.vrchat_login.allowed = true;
        h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        click(&mut h, "Play in VR");
        let_pass(&mut h, 60);
        let sent = starts(&h);
        assert!(matches!(sent.as_slice(), [Action::PlayAt { destination, .. }] if destination.instance_id == visit.instance), "{sent:?}");
    }

    /// verify2-r21 M3: a card chosen before, then a pick being checked when the sheet is put away: Play never launches
    /// the earlier card.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_a_pending_pick_is_what_play_goes_to_not_the_card_before() {
        use orbiters_core::launcher::{Action, InviteKind};
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(31, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        let invite = h.state.launcher_mut().data.invites.iter().find(|i| i.kind == InviteKind::Invite).cloned().unwrap();
        let invite_world = invite.place.as_ref().unwrap().world_name.clone();
        click(&mut h, &invite_world);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(format!("invite:{}", invite.id).as_str()));
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(1500)));
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        try_click(&mut h, "Play in VR");
        let_pass(&mut h, 2200);
        let sent = starts(&h);
        assert!(sent.iter().all(|a| matches!(a, Action::PlayAt { destination, .. } if destination.instance_id == visit.instance)), "the earlier card: {sent:?}");
    }

    /// verify2-r21 M3: the profile chosen changes while Play's recheck is under way: nothing starts when the answer comes.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_a_profile_change_during_a_check_launches_nothing() {
        use orbiters_core::launcher::GameProfile;
        let visit = row_of(41, "Harbor Market");
        let mut h = chosen_and_aged(vec![visit.clone()]);
        let mut data = (*h.state.launcher_mut().data).clone();
        let plain = |profile| GameProfile { profile, account: None, running: None, starting: None, assistant: false };
        data.profiles = vec![plain(0), plain(1)];
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        h.state.launcher_mut().places.desk.age_check(Duration::from_secs(65));
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(500)));
        click(&mut h, "Play in VR");
        h.state.launcher_mut().stand_in_profile(1);
        let_pass(&mut h, 1500);
        assert!(starts(&h).is_empty(), "started on the profile left: {:?}", starts(&h));
        assert!(h.state.launcher_mut().places.making().is_none(), "busy ended");
    }

    /// verify2-r21 M3: an answer asked for one account is never an "open" for another: it is asked again as the new one.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_an_account_change_during_a_check_asks_again() {
        use orbiters_core::launcher::Source;
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(51, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(400)));
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        // The other account signs in before the answer comes, its friends list with it.
        let other = "usr_00000000-0000-4000-8000-0000000000ee";
        let mut data = (*h.state.launcher_mut().data).clone();
        let Source::SignedIn { name, .. } = data.source.clone() else { panic!() };
        data.source = Source::SignedIn { user_id: other.into(), name: name.clone() };
        h.state.launcher_mut().set_data(Arc::new(data));
        h.snap.vrchat_login.user = Some(vrchat_user(other, &name));
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        assert!(instance_checks(&h) >= 2, "the answer for the account before counted for this one");
    }

    /// verify2-r21 M3: VRChat starts running on the profile during Play's recheck (Where hides); when it stops, nothing is
    /// launched for the press made long before.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_a_press_is_not_honoured_after_where_was_hidden() {
        use orbiters_core::launcher::{Game, GameProfile, LaunchMode};
        let visit = row_of(61, "Harbor Market");
        let mut h = chosen_and_aged(vec![visit.clone()]);
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(300)));
        click(&mut h, "Play in VR");
        let before = h.state.launcher_mut().data.clone();
        let mut data = (*before).clone();
        data.game = Game::Running(LaunchMode::Vr);
        data.profiles = vec![GameProfile { profile: 0, account: None, running: Some(LaunchMode::Vr), starting: None, assistant: false }];
        h.state.launcher_mut().set_data(Arc::new(data));
        let_pass(&mut h, 700);
        h.state.launcher_mut().set_data(before);
        let_pass(&mut h, 400);
        assert!(starts(&h).is_empty(), "launched for an old press: {:?}", starts(&h));
        assert!(h.state.launcher_mut().places.making().is_none(), "busy ended");
    }

    /// verify2-r21 M3: Play pressed twice, and again while its recheck is under way, starts VRChat once.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_playing_twice_or_during_a_recheck_starts_once() {
        use orbiters_core::launcher::Action;
        let visit = row_of(71, "Harbor Market");
        let mut h = chosen_and_aged(vec![visit.clone()]);
        h.recorder.alive_by.lock().unwrap().insert(visit.instance.clone(), (Some(true), Duration::from_millis(300)));
        let (r, _) = orbiters_ui::widgets::control(&h.ctx, "Play in VR").unwrap();
        for _ in 0..2 {
            h.press(r.center());
            h.release(r.center());
        }
        try_click(&mut h, "Play in Desktop");
        let_pass(&mut h, 900);
        let sent = starts(&h);
        assert!(matches!(sent.as_slice(), [Action::PlayAt { destination, mode: orbiters_core::launcher::LaunchMode::Vr, .. }] if destination.instance_id == visit.instance), "{sent:?}");
        // A fresh "open", pressed twice in a row: once too.
        let (r, _) = orbiters_ui::widgets::control(&h.ctx, "Play in VR").unwrap();
        for _ in 0..60 {
            h.frame(Vec::new());
        }
        for _ in 0..2 {
            h.press(r.center());
            h.release(r.center());
        }
        let_pass(&mut h, 100);
        assert_eq!(starts(&h).len(), 2, "one more: {:?}", starts(&h));
    }

    /// verify2-r21 M3: an instance VRChat says is closed stays what is chosen, Play starts nothing (greyed, and says why),
    /// and the line offers a new instance of its world; one VRChat did not say anything of is asked about again by Play.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_a_closed_choice_holds_play_and_an_unknown_one_is_asked_again() {
        use orbiters_core::launcher::Action;
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(81, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        *h.recorder.alive.lock().unwrap() = Some(false);
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "answered", |h| h.state.launcher_mut().places.notice().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        assert_eq!(h.state.launcher_mut().places.selected_key(), Some(visit.key().as_str()), "still what is chosen");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None, "and not gone to");
        assert!(!try_click(&mut h, "Play in VR"), "greyed");
        let_pass(&mut h, 50);
        assert!(starts(&h).is_empty(), "{:?}", starts(&h));
        // The line under Where: a new instance of its world, made at Play.
        click(&mut h, "New instance of this world");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(visit.key().as_str()));
        click(&mut h, "Play in VR");
        frames_until(&mut h, "made and gone to", |h| !starts(h).is_empty());
        assert!(matches!(starts(&h).as_slice(), [Action::PlayAt { destination, .. }] if destination.instance_id == MADE), "{:?}", starts(&h));
        // Home world chosen, then the card it left: asked about again, VRChat does not say.
        click(&mut h, "Home world");
        *h.recorder.alive.lock().unwrap() = None;
        let before = instance_checks(&h);
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "not said", |h| h.state.launcher_mut().places.notice().is_some_and(|n| n.contains("did not say")));
        assert_eq!(instance_checks(&h), before + 1);
        // Play asks again; open by now: there.
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Play in Desktop");
        frames_until(&mut h, "gone to", |h| starts(h).len() == 2);
        assert_eq!(instance_checks(&h), before + 2);
        assert!(matches!(starts(&h).last(), Some(Action::PlayAt { destination, .. }) if destination.instance_id == visit.instance), "{:?}", starts(&h));
    }

    /// verify2-r21 M3: the last instance's card goes by the same rule: its "open" counts for a minute, then Play asks.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify2_the_last_instances_card_is_asked_again_once_its_open_is_old() {
        use crate::pages::launcher::destinations::{HistoryView, LastVisit};
        use crate::pages::launcher::previous::OPEN_FOR;
        use orbiters_core::launcher::Action;
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(91, "Harbor Market");
        let mut h = where_signed_in(Vec::new());
        let last = LastVisit { world_id: visit.world_id.clone(), instance: visit.instance.clone(), name: visit.name.clone(), at: visit.at, picture: None };
        h.state.launcher_mut().places.stand_in(Some(HistoryView { frequent: Vec::new(), last: Some(last) }), Vec::new(), Some(true), true);
        h.settle();
        click(&mut h, "Harbor Market");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(format!("recent:{}", visit.world_id).as_str()));
        click(&mut h, "Play in VR");
        assert_eq!((starts(&h).len(), instance_checks(&h)), (1, 0), "said open a moment ago: {:?}", starts(&h));
        h.state.launcher_mut().places.desk.age_check(OPEN_FOR + Duration::from_secs(5));
        *h.recorder.alive.lock().unwrap() = Some(false);
        click(&mut h, "Play in VR");
        frames_until(&mut h, "asked", |h| instance_checks(h) == 1);
        let_pass(&mut h, 100);
        assert_eq!(starts(&h).len(), 1, "closed meanwhile: nothing starts");
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None);
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Home world");
        click(&mut h, "Harbor Market");
        click(&mut h, "Play in Desktop");
        frames_until(&mut h, "gone to", |h| starts(h).len() == 2);
        assert!(matches!(starts(&h).last(), Some(Action::PlayAt { destination, .. }) if destination.instance_id == visit.instance));
    }

    /// REQUEST23: the sample's friends with "Nova Sample" in a friends+ instance (joinable without asking) and "Pixel Fox"
    /// in someone else's invite instance (not), Where signed in (or `connecting`).
    fn friends_in_instances(connecting: bool) -> (Harness, orbiters_core::launcher::Place, String) {
        use orbiters_core::launcher::{InstanceAccess, Place, Presence, Source, Whereabouts};
        let mut h = Harness::new();
        let mut data = orbiters_core::launcher::sample();
        let Source::SignedIn { user_id: me, name } = data.source.clone() else { panic!("the sample is signed in") };
        let joinable = Place {
            world_id: "wrld_00000000-0000-4000-8000-000000000a01".into(),
            world_name: "Lantern Garden".into(),
            world_picture: String::new(),
            instance_id: "77123~hidden(usr_00000000-0000-4000-8000-0000000000aa)~region(eu)".into(),
            access: InstanceAccess::FriendsPlus,
            region: "eu".into(),
        };
        let invite = Place { instance_id: "88~private(usr_00000000-0000-4000-8000-0000000000bb)~region(eu)".into(), access: InstanceAccess::Invite, ..joinable.clone() };
        for f in data.friends.iter_mut() {
            if f.name == "Nova Sample" {
                (f.presence, f.whereabouts) = (Presence::InGame, Whereabouts::World(joinable.clone()));
            }
            if f.name == "Pixel Fox" {
                (f.presence, f.whereabouts) = (Presence::InGame, Whereabouts::World(invite.clone()));
            }
        }
        {
            let l = h.state.launcher_mut();
            l.set_data(Arc::new(data));
            l.stand_in(1_795_000_000, "");
            l.places.stand_in(Some(crate::pages::launcher::destinations::HistoryView::default()), Vec::new(), None, true);
        }
        if connecting {
            h.snap.vrchat_login.restoring = true;
            h.snap.vrchat_login.known = Some(vrchat_user(&me, &name));
        } else {
            h.snap.vrchat_login.allowed = true;
            h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
        }
        h.settle();
        (h, joinable, me)
    }

    fn friend_id(h: &mut Harness, name: &str) -> String {
        h.state.launcher_mut().data_for_tests().friends.iter().find(|f| f.name == name).unwrap().id.clone()
    }

    fn open_profile_of(h: &mut Harness, name: &str) {
        let id = friend_id(h, name);
        let ctx = h.ctx.clone();
        h.state.launcher_mut().open_profile(&ctx, &id, Rect::from_min_size(pos2(800.0, 300.0), vec2(200.0, 50.0)));
        h.settle();
    }

    /// REQUEST23: a friend in an instance that takes the account without asking shows Select where Request was; Select
    /// asks nothing of the account (no invite request, no start): the instance becomes Where's choice (once VRChat says
    /// it is open, the existing check), Play starts VRChat in it exactly as the friend's location names it, and when the
    /// friend leaves it the choice goes back to Home and the row says so. Someone else's invite instance keeps Request.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn select_on_a_friend_chooses_their_instance_in_where() {
        use orbiters_core::launcher::{Action, Presence, Whereabouts};
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, _) = friends_in_instances(false);
        open_profile_of(&mut h, "Pixel Fox");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Request invite").is_some(), "an invite instance: Request stays");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Select").is_none());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        open_profile_of(&mut h, "Nova Sample");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Request invite").is_none(), "Select in its place");
        let before = h.recorder.launcher.lock().unwrap().len();
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Select");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        let key_of = format!("previous:{}:{}", place.world_id, place.instance_id);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(key_of.as_str()));
        let sent: Vec<_> = h.recorder.launcher.lock().unwrap()[before..].to_vec();
        let close_only = |a: &Action| matches!(a, Action::CloseProfile) || matches!(a, Action::As { action, .. } if matches!(**action, Action::CloseProfile));
        assert!(sent.iter().all(close_only), "Select asks nothing of the account: {sent:?}");
        assert_eq!(h.recorder.asked.lock().unwrap().as_slice(), [format!("InstanceSeen {{ world: {:?}, instance: {:?}, max_age_ms: 30000 }}", place.world_id, place.instance_id)], "only the existing open check");
        assert!(h.state.launcher_mut().modal_open().is_none(), "back to Home, Where in view");
        click(&mut h, "Play in VR");
        let last = match h.recorder.launcher.lock().unwrap().last().cloned() {
            Some(Action::As { action, .. }) => *action,
            Some(other) => other,
            None => unreachable!(),
        };
        let Action::PlayAt { destination, .. } = last else { panic!("{last:?}") };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (place.world_id.as_str(), place.instance_id.as_str()));
        // The friend leaves for another instance: the choice goes back to Home, said quietly.
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        for f in data.friends.iter_mut().filter(|f| f.name == "Nova Sample") {
            (f.presence, f.whereabouts) = (Presence::InGame, Whereabouts::Private);
        }
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        assert_eq!(h.state.launcher_mut().places.chosen_key(), None);
        assert!(h.state.launcher_mut().places.notice().is_some_and(|n| n.contains("Nova Sample")), "{:?}", h.state.launcher_mut().places.notice());
    }

    /// REQUEST23: the same on a friend's row of the list, under the pointer: Select where Request was (its pill as wide
    /// as Request's), and a press chooses the instance without opening the profile or sending anything.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_friends_row_selects_their_instance() {
        use orbiters_core::launcher::Action;
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, _) = friends_in_instances(false);
        // The row's pill appears under the pointer: down the side panel until a row shows `label`.
        let over = |h: &mut Harness, label: &str| {
            for y in (60..760).step_by(12) {
                for _ in 0..12 {
                    h.frame(vec![Event::PointerMoved(pos2(800.0, y as f32))]);
                }
                if let Some((r, _)) = orbiters_ui::widgets::control(&h.ctx, label).filter(|(r, _)| r.left() > 721.0) {
                    return r;
                }
            }
            panic!("no row shows {label}");
        };
        let request = over(&mut h, "Request");
        let select = over(&mut h, "Select");
        assert_eq!(select.width(), request.width(), "no jump between the two");
        let before = h.recorder.launcher.lock().unwrap().len();
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Select");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        let key_of = format!("previous:{}:{}", place.world_id, place.instance_id);
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(key_of.as_str()));
        assert!(h.recorder.launcher.lock().unwrap()[before..].iter().all(|a| !matches!(a, Action::As { .. })), "nothing asked of the account");
        assert!(h.state.launcher_mut().modal_open().is_none(), "the row's profile did not open");
    }

    /// REQUEST23: VRChat saying the friend's instance is closed, or not answering, chooses nothing and the line under
    /// Where says which (no sheet's row does); on another account's profile, Request stays.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn select_on_a_closed_instance_chooses_nothing_and_says_so() {
        use orbiters_core::launcher::Action;
        orbiters_ui::widgets::watch_controls();
        for (open, said) in [(Some(false), "closed"), (None, "did not say")] {
            let (mut h, place, _) = friends_in_instances(false);
            // The answer held back a moment: until it comes the line says it is being asked.
            h.recorder.alive_by.lock().unwrap().insert(place.instance_id.clone(), (open, Duration::from_millis(1000)));
            open_profile_of(&mut h, "Nova Sample");
            click(&mut h, "Select");
            assert!(h.state.launcher_mut().places.checking_line().is_some_and(|l| l.contains("Nova Sample")), "being asked: said");
            frames_until(&mut h, "answered", |h| h.state.launcher_mut().places.notice().is_some());
            let line = h.state.launcher_mut().places.notice().unwrap();
            assert!(line.contains("Nova Sample") && line.contains(said), "{line}");
            assert_eq!(h.state.launcher_mut().places.chosen_key(), None);
            assert!(h.state.launcher_mut().places.checking_line().is_none());
            assert!(h.recorder.launcher.lock().unwrap().iter().all(|a| !matches!(a, Action::PlayAt { .. })));
        }
        // Another account's profile chosen under Play: this account cannot start VRChat there, Request as before.
        let (mut h, _, _) = friends_in_instances(false);
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        data.profiles = vec![orbiters_core::launcher::GameProfile { profile: 0, account: Some("Someone Else".into()), running: None, starting: None, assistant: false }];
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        open_profile_of(&mut h, "Nova Sample");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Select").is_none(), "no Select on another account's profile");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Request invite").is_some());
    }

    /// verify3-r21 M3 (the verifier's own fixture): the friend left before the window takes the answer Play waits for, in
    /// the same frame: nothing starts.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify3_friend_departure_before_consuming_open_cancels_play() {
        use orbiters_core::launcher::{Presence, Whereabouts};
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, _) = friends_in_instances(false);
        open_profile_of(&mut h, "Nova Sample");
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Select");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.state.launcher_mut().places.desk.age_check(Duration::from_secs(65));
        h.recorder.alive_by.lock().unwrap().insert(place.instance_id.clone(), (Some(true), Duration::from_millis(1000)));
        click(&mut h, "Play in VR");
        assert!(h.state.launcher_mut().places.desk.rechecking(), "fixture armed");
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        for f in data.friends.iter_mut().filter(|f| f.name == "Nova Sample") {
            (f.presence, f.whereabouts) = (Presence::InGame, Whereabouts::Private);
        }
        h.state.launcher_mut().set_data(Arc::new(data));
        std::thread::sleep(Duration::from_millis(1200));
        h.frame(Vec::new());
        assert!(starts(&h).is_empty(), "a launch was sent in the departure frame: {:?}", starts(&h));
    }

    /// verify3-r21 M2: an "open" the service kept counts from when VRChat gave it: nearly a minute old at its arrival, it is
    /// asked about again a few seconds later; the last instance's card too, for the profile chosen now only.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify3_an_open_the_service_kept_counts_from_when_vrchat_gave_it() {
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(1201, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        *h.recorder.alive.lock().unwrap() = Some(true);
        *h.recorder.age_ms.lock().unwrap() = 59_000;
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        assert_eq!(instance_checks(&h), 1);
        std::thread::sleep(Duration::from_millis(1300));
        // Answered from what was kept once more, past the minute by now: nothing starts from it.
        *h.recorder.age_ms.lock().unwrap() = 61_000;
        click(&mut h, "Play in VR");
        frames_until(&mut h, "asked again", |h| instance_checks(h) == 2);
        let_pass(&mut h, 200);
        assert!(starts(&h).is_empty(), "an answer older than a minute starts nothing: {:?}", starts(&h));
        // VRChat's own answer, now: there.
        *h.recorder.age_ms.lock().unwrap() = 0;
        click(&mut h, "Play in VR");
        frames_until(&mut h, "started", |h| starts(h).len() == 1);
    }

    /// verify3-r21 M1: a destination verified for an account goes to the service bound to that account, also while the
    /// friends list is not read yet: the service can tell a switch on the way.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn verify3_a_verified_destination_keeps_its_account_to_the_service() {
        use orbiters_core::launcher::{Action, Source};
        orbiters_ui::widgets::watch_controls();
        let visit = row_of(1101, "Harbor Market");
        let mut h = where_signed_in(vec![visit.clone()]);
        let Source::SignedIn { user_id: me, .. } = h.state.launcher_mut().data.source.clone() else { panic!() };
        // Signed in, its list still to come.
        let mut data = (*h.state.launcher_mut().data).clone();
        data.source = Source::Nothing;
        data.friends.clear();
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Previous instances");
        click(&mut h, "Harbor Market");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        h.frame(vec![key(Key::Escape)]);
        h.settle();
        click(&mut h, "Play in VR");
        let sent = h.recorder.launcher.lock().unwrap().clone();
        assert!(matches!(sent.last(), Some(Action::As { account, action }) if *account == me && matches!(&**action, Action::PlayAt { destination, .. } if destination.instance_id == visit.instance)), "{sent:?}");
    }

    // --- REQUEST23, the owner: "the world thumbnail doesn't load": the place chosen has its world's picture. ---

    /// A made-up picture address of VRChat's (each test its own: what was read is kept on disk by address).
    #[cfg(not(target_os = "macos"))]
    fn thumb_url(n: u32) -> String {
        format!("https://api.vrchat.cloud/api/1/image/file_00000000-0000-4000-8000-{n:012}/1/256")
    }

    /// A made-up picture.
    #[cfg(not(target_os = "macos"))]
    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(16, 9, image::Rgba([40, 120, 200, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[cfg(not(target_os = "macos"))]
    fn pictures_asked(h: &Harness) -> Vec<String> {
        h.recorder.asked.lock().unwrap().iter().filter(|a| a.starts_with("Image")).cloned().collect()
    }

    /// The friend `name` is in `world`, whose picture the friends list gives as `url`.
    #[cfg(not(target_os = "macos"))]
    fn puts_in(h: &mut Harness, name: &str, place: &orbiters_core::launcher::Place, world: u32, url: &str) -> orbiters_core::launcher::Place {
        use orbiters_core::launcher::{Presence, Whereabouts};
        let place = orbiters_core::launcher::Place { world_id: format!("wrld_00000000-0000-4000-8000-{world:012}"), world_picture: url.into(), ..place.clone() };
        let mut data = (*h.state.launcher_mut().data).clone();
        for f in data.friends.iter_mut().filter(|f| f.name == name) {
            (f.presence, f.whereabouts) = (Presence::InGame, Whereabouts::World(place.clone()));
        }
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        place
    }

    /// The friend's row under the pointer, its Select pressed (their profile, which reads their picture, not opened).
    #[cfg(not(target_os = "macos"))]
    fn select_on_row(h: &mut Harness, name: &str) {
        for y in (60..760).step_by(12) {
            for _ in 0..12 {
                h.frame(vec![Event::PointerMoved(pos2(800.0, y as f32))]);
            }
            if orbiters_ui::widgets::control(&h.ctx, "Select").is_some_and(|(r, _)| r.left() > 721.0) {
                return click(h, "Select");
            }
        }
        panic!("no row shows Select ({name}'s is the first that can)");
    }

    /// Selecting a friend's instance asks VRChat whether it is open and for that one world's picture, once, at the address
    /// the friends list already has; nothing before the press.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn selecting_a_friends_instance_asks_for_its_worlds_picture_once() {
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, _) = friends_in_instances(false);
        let url = thumb_url(1);
        let place = puts_in(&mut h, "Nova Sample", &place, 2001, &url);
        h.recorder.images.lock().unwrap().insert(url.clone(), (Some(png()), Duration::from_millis(150)));
        *h.recorder.alive.lock().unwrap() = Some(true);
        for _ in 0..30 {
            h.frame(Vec::new());
        }
        assert!(pictures_asked(&h).is_empty(), "nothing before the press");
        select_on_row(&mut h, "Nova Sample");
        frames_until(&mut h, "chosen", |h| h.state.launcher_mut().places.chosen_key().is_some());
        let_pass(&mut h, 500);
        assert_eq!(pictures_asked(&h), [format!("Image {{ url: {url:?} }}")]);
        assert_eq!(h.recorder.asked.lock().unwrap().len(), 2, "that and whether it is open, nothing else: {:?}", h.recorder.asked.lock().unwrap());
        assert_eq!(h.state.launcher_mut().places.chosen_key(), Some(format!("previous:{}:{}", place.world_id, place.instance_id).as_str()));
    }

    /// While its picture is read the chosen card has the loading placeholder, then the picture; one that cannot be had is
    /// the usual card, and is not asked for again and again.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_chosen_cards_picture_loads_shows_or_falls_back() {
        orbiters_ui::widgets::watch_controls();
        for (n, bytes) in [(11, Some(png())), (12, None)] {
            let (mut h, place, _) = friends_in_instances(false);
            let url = thumb_url(n);
            let place = puts_in(&mut h, "Nova Sample", &place, 2000 + n, &url);
            let ends = if bytes.is_some() { "ready" } else { "none" };
            h.recorder.images.lock().unwrap().insert(url, (bytes, Duration::from_millis(500)));
            *h.recorder.alive.lock().unwrap() = Some(true);
            select_on_row(&mut h, "Nova Sample");
            let key = format!("previous:{}:{}", place.world_id, place.instance_id);
            assert_eq!(h.state.launcher_mut().places.picture_of(&key), Some("loading"));
            frames_until(&mut h, ends, |h| h.state.launcher_mut().places.picture_of(&key) == Some(ends));
            let_pass(&mut h, 300);
            assert_eq!(h.state.launcher_mut().places.picture_of(&key), Some(ends));
            assert_eq!(pictures_asked(&h).len(), 1, "{:?}", pictures_asked(&h));
            // Home chosen, its card still offered: what is here stays, nothing more is asked.
            click(&mut h, "Home world");
            let_pass(&mut h, 200);
            assert_eq!(h.state.launcher_mut().places.picture_of(&key), Some(ends));
            assert_eq!(pictures_asked(&h).len(), 1, "a card not chosen asks for nothing");
        }
    }

    /// A choice replaced before its picture came: the picture is not the next choice's, and the next one asks for its own.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_late_picture_is_not_the_next_choices() {
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, _) = friends_in_instances(false);
        let (a, b) = (thumb_url(21), thumb_url(22));
        puts_in(&mut h, "Nova Sample", &place, 2021, &a);
        h.recorder.images.lock().unwrap().insert(a.clone(), (Some(png()), Duration::from_millis(400)));
        // The second one's fails: whatever it shows is not the first one's.
        h.recorder.images.lock().unwrap().insert(b.clone(), (None, Duration::ZERO));
        *h.recorder.alive.lock().unwrap() = Some(true);
        select_on_row(&mut h, "Nova Sample");
        let place = puts_in(&mut h, "Nova Sample", &place, 2022, &b);
        select_on_row(&mut h, "Nova Sample");
        let key = format!("previous:{}:{}", place.world_id, place.instance_id);
        frames_until(&mut h, "the next choice", |h| h.state.launcher_mut().places.chosen_key() == Some(key.as_str()));
        let_pass(&mut h, 800);
        assert_eq!(h.state.launcher_mut().places.picture_of(&key), Some("none"));
        assert_eq!(pictures_asked(&h), [format!("Image {{ url: {a:?} }}"), format!("Image {{ url: {b:?} }}")], "each once");
    }

    /// The other cards: a previous instance picked has its world's picture, at the address a friend there brought (none
    /// asked while the list is browsed); so have a place visited often and an invite once chosen, each asked once.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn every_kind_of_card_has_its_picture_once_chosen() {
        use crate::pages::launcher::destinations::{Frequent, HistoryView};
        use orbiters_core::launcher::InviteKind;
        orbiters_ui::widgets::watch_controls();
        // A previous instance: browsing asks for nothing, the pick for the open check and one picture.
        let rows: Vec<_> = (0..6).map(|n| row_of(1000 + n, &format!("Quiet World {n}"))).collect();
        let mut h = where_signed_in(rows.clone());
        let friend_place = orbiters_core::launcher::Place { world_id: rows[0].world_id.clone(), world_name: "Quiet World 0".into(), world_picture: String::new(), instance_id: "9~region(eu)".into(), access: orbiters_core::launcher::InstanceAccess::Public, region: "eu".into() };
        let url = thumb_url(31);
        {
            let mut data = (*h.state.launcher_mut().data).clone();
            let f = data.friends.iter_mut().find(|f| f.name == "Nova Sample").unwrap();
            f.whereabouts = orbiters_core::launcher::Whereabouts::World(orbiters_core::launcher::Place { world_picture: url.clone(), ..friend_place });
            h.state.launcher_mut().set_data(Arc::new(data));
        }
        h.recorder.images.lock().unwrap().insert(url.clone(), (Some(png()), Duration::ZERO));
        *h.recorder.alive.lock().unwrap() = Some(true);
        click(&mut h, "Previous instances");
        let_pass(&mut h, 150);
        assert!(h.recorder.asked.lock().unwrap().is_empty(), "browsing sends nothing: {:?}", h.recorder.asked.lock().unwrap());
        click(&mut h, "Quiet World 0");
        frames_until(&mut h, "its picture", |h| h.state.launcher_mut().places.picture_of(&rows[0].key()) == Some("ready"));
        let_pass(&mut h, 200);
        assert_eq!((instance_checks(&h), pictures_asked(&h)), (1, vec![format!("Image {{ url: {url:?} }}")]));
        assert_eq!(h.recorder.asked.lock().unwrap().len(), 2, "{:?}", h.recorder.asked.lock().unwrap());
        // A place visited often, and an invite: their world's, once chosen.
        let mut h = where_signed_in(Vec::new());
        let mut data = (*h.state.launcher_mut().data).clone();
        let invite = data.invites.iter().find(|i| i.kind == InviteKind::Invite).cloned().unwrap();
        let invited = invite.place.clone().unwrap();
        let (often, often_url, invite_url) = ("wrld_00000000-0000-4000-8000-000000002032", thumb_url(32), thumb_url(33));
        let mut there = data.friends.iter_mut();
        for (world, url) in [(often.to_string(), &often_url), (invited.world_id.clone(), &invite_url)] {
            there.next().unwrap().whereabouts = orbiters_core::launcher::Whereabouts::World(orbiters_core::launcher::Place { world_id: world, world_picture: url.clone(), instance_id: "9~private(usr_x)~region(eu)".into(), ..invited.clone() });
        }
        h.state.launcher_mut().set_data(Arc::new(data));
        let view = HistoryView { frequent: vec![Frequent { world_id: often.into(), name: "Lantern Garden".into(), visits: 5, score: 3.0, last: 0, picture: None }], last: None };
        h.state.launcher_mut().places.stand_in(Some(view), Vec::new(), None, true);
        h.settle();
        assert!(pictures_asked(&h).is_empty(), "nothing before a choice: {:?}", pictures_asked(&h));
        for (n, (name, key, url)) in [("Lantern Garden".to_string(), format!("often:{often}"), often_url), (invited.world_name.clone(), format!("invite:{}", invite.id), invite_url)].into_iter().enumerate() {
            h.recorder.images.lock().unwrap().insert(url.clone(), (Some(png()), Duration::ZERO));
            click(&mut h, &name);
            frames_until(&mut h, &name, |h| h.state.launcher_mut().places.picture_of(&key) == Some("ready"));
            let_pass(&mut h, 100);
            assert_eq!(pictures_asked(&h).len(), n + 1, "{:?}", pictures_asked(&h));
        }
    }

    /// REQUEST23: while VRChat's session is restored, Select holds the friend's instance (nothing chosen, nothing asked)
    /// until VRChat says who is signed in; with VRChat running on the profile chosen (Where hidden), the profile keeps
    /// Request invite as before.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn select_waits_while_vrchat_connects_and_is_not_offered_while_it_runs() {
        use orbiters_core::launcher::{Game, GameProfile, LaunchMode, Source};
        orbiters_ui::widgets::watch_controls();
        let (mut h, place, me) = friends_in_instances(true);
        open_profile_of(&mut h, "Nova Sample");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Select").is_some(), "Select offered while connecting (held)");
        {
            click(&mut h, "Select");
            assert_eq!(h.state.launcher_mut().places.chosen_key(), None, "held while connecting");
            assert!(h.state.launcher_mut().places.checking_line().is_some_and(|l| l.contains("Nova Sample")), "said under Where");
            assert!(h.recorder.asked.lock().unwrap().is_empty());
            *h.recorder.alive.lock().unwrap() = Some(true);
            let name = h.snap.vrchat_login.known.clone().unwrap().display_name;
            h.snap.vrchat_login.restoring = false;
            h.snap.vrchat_login.known = None;
            h.snap.vrchat_login.allowed = true;
            h.snap.vrchat_login.user = Some(vrchat_user(&me, &name));
            frames_until(&mut h, "chosen once verified", |h| h.state.launcher_mut().places.chosen_key().is_some());
            assert!(h.recorder.asked.lock().unwrap()[0].contains(&place.instance_id));
        }
        // VRChat running on the profile chosen: Where is hidden, Request invite as before.
        let (mut h, _, _) = friends_in_instances(false);
        let mut data = (*h.state.launcher_mut().data_for_tests()).clone();
        let Source::SignedIn { name, .. } = data.source.clone() else { unreachable!() };
        data.game = Game::Running(LaunchMode::Vr);
        data.profiles = vec![GameProfile { profile: 0, account: Some(name), running: Some(LaunchMode::Vr), starting: None, assistant: false }];
        h.state.launcher_mut().set_data(Arc::new(data));
        h.settle();
        open_profile_of(&mut h, "Nova Sample");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Select").is_none(), "no Select while VRChat runs on the profile");
        assert!(orbiters_ui::widgets::control(&h.ctx, "Request invite").is_some());
    }
}
