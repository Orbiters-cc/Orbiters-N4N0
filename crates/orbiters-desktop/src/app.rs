//! The window: glow background, frameless title bar, sidebar with a sliding pill, the pages with spring
//! entrances, toasts and the opening splash. The tray service does the work; this only reads its snapshot.

use crate::backend::Ctl;
use crate::controller::{Notice, NoticeKind};
use crate::ipc::UpdateAction;
use crate::pages::{self, Page};
use crate::state::{AccountState, CaptureState, Snapshot, UpdateState};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, ResizeDirection, Sense, Ui, Vec2, ViewportCommand};
use orbiters_i18n::{mark, tr, tr_n};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::glow::{self, Mood};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, n4n0_logo, toast, widgets};
use std::sync::Arc;

pub(crate) const TITLE_H: f32 = 40.0;
const SIDEBAR_W: f32 = 232.0;
/// The splash shows for this long, then fades out over [`SPLASH_FADE`].
const SPLASH: f64 = 1.9;
const SPLASH_FADE: f64 = 0.55;

pub struct App {
    ctl: Ctl,
    /// False offscreen (screenshots): no window commands.
    windowed: bool,
    gpu: Option<crate::gpu::Handles>,
    page: Page,
    page_since: f64,
    visit: u64,
    started: Option<f64>,
    show_splash: bool,
    sidebar_since: Option<f64>,
    toasts: toast::Toasts,
    /// The avatar gun card's picture of the avatar (`crate::avatar_gun_ui`), made at its first card.
    gun_pictures: Option<crate::worlds::Pictures>,
    home: pages::home::State,
    capture: pages::capture::State,
    projects: pages::projects::State,
    sessions: pages::sessions::State,
    worlds: pages::worlds::State,
    events: pages::events::State,
    videos: pages::videos::State,
    settings: pages::settings::State,
    n4n0: pages::n4n0::State,
    /// N4N0's bar in the title strip and the chat it grows into (REQUEST21 3).
    chat: crate::chat_bar::ChatBar,
    logo_glitch: Option<(f64, f32)>,
    logo_hovered: bool,
    /// What lies behind the pages.
    wallpaper: crate::appearance::Wallpaper,
    /// The page (and Home's feature) last told to the usage statistics.
    usage_here: Option<(Page, Option<orbiters_n4n0_core::menu::Page>)>,
    /// The usage statistics' notice was closed here (the service's settings say so a moment later).
    usage_told: bool,
    /// Minimised for long while VRChat runs: the countdown to closing to the tray (`crate::window_rest`), on its clock.
    #[cfg(target_os = "macos")]
    cover: crate::mac_cover::Cover,
    /// What was last told of the service's state that the window shows.
    #[cfg(target_os = "macos")]
    told: Option<Told>,
    rest: crate::window_rest::Rest,
    rest_clock: std::time::Instant,
    #[cfg(windows)]
    rest_card: crate::window_rest::card::Card,
}

/// A Mac window's three buttons, in macOS's own colours, and the room they take at the title bar's left.
const MAC_CLOSE: egui::Color32 = egui::Color32::from_rgb(0xff, 0x5f, 0x57);
const MAC_MINIMIZE: egui::Color32 = egui::Color32::from_rgb(0xfe, 0xbc, 0x2e);
const MAC_FULL_SCREEN: egui::Color32 = egui::Color32::from_rgb(0x28, 0xc8, 0x40);
const MAC_BUTTONS_W: f32 = 82.0;

impl App {
    /// The window of the window process; `splash` plays the opening (first window since the app started).
    pub fn new(cc: &eframe::CreationContext<'_>, ctl: Ctl, splash: bool) -> Self {
        orbiters_ui::install(&cc.egui_ctx);
        if let Some(rs) = &cc.wgpu_render_state {
            crate::gpu::report_errors(&rs.device);
            orbiters_ui::install_wgpu(&rs.device, rs.target_format, 1, &mut rs.renderer.write().callback_resources);
        }
        ctl.attach(&cc.egui_ctx);
        pages::wake_when_work_ends(&cc.egui_ctx);
        #[cfg(windows)]
        round_corners(cc);
        #[cfg(target_os = "macos")]
        crate::mac::install_window_menu(ctl.clone());
        #[cfg(target_os = "macos")]
        crate::mac_cover::watch(&cc.egui_ctx);
        let mut app = Self::offline(ctl);
        app.windowed = true;
        app.gpu = cc.wgpu_render_state.as_ref().map(crate::gpu::Handles::of_window);
        app.show_splash = splash;
        // Closed to the tray to save memory: back on the page it was on.
        if let Some(name) = crate::window_rest::resume::take() {
            app.go_named(&name, 0.0);
        }
        app
    }

    /// An app without a window (screenshots).
    pub fn offline(ctl: Ctl) -> Self {
        Self {
            ctl,
            windowed: false,
            gpu: None,
            page: Page::Home,
            page_since: 0.0,
            visit: 0,
            started: None,
            show_splash: true,
            sidebar_since: None,
            toasts: toast::Toasts::default(),
            gun_pictures: None,
            home: Default::default(),
            capture: Default::default(),
            projects: Default::default(),
            sessions: Default::default(),
            worlds: Default::default(),
            events: Default::default(),
            videos: Default::default(),
            settings: Default::default(),
            n4n0: Default::default(),
            chat: Default::default(),
            logo_glitch: None,
            logo_hovered: false,
            wallpaper: Default::default(),
            usage_here: None,
            usage_told: false,
            #[cfg(target_os = "macos")]
            cover: Default::default(),
            #[cfg(target_os = "macos")]
            told: None,
            rest: Default::default(),
            rest_clock: std::time::Instant::now(),
            #[cfg(windows)]
            rest_card: Default::default(),
        }
    }

    pub fn skip_splash(&mut self) {
        self.show_splash = false;
    }

    pub fn set_page(&mut self, page: Page, now: f64) {
        if self.page == Page::Settings && page != Page::Settings {
            self.settings.stop_mic_test();
        }
        if self.page == Page::N4n0 && page != Page::N4n0 {
            self.n4n0.leave(&self.ctl);
        }
        if self.page == Page::Home && page != Page::Home {
            self.home.left(&self.ctl);
        }
        // Home again while a feature's page shows: back to Home itself.
        if page == Page::Home && self.page == Page::Home {
            self.home.back_home();
        }
        if page != self.page || self.visit == 0 {
            self.page = page;
            self.page_since = now;
            self.visit += 1;
        }
    }

    /// The GPU of an offscreen renderer (screenshots).
    pub fn set_gpu(&mut self, gpu: crate::gpu::Handles) {
        self.gpu = Some(gpu);
    }

    /// The page shown.
    pub fn shown_page(&self) -> Page {
        self.page
    }

    /// N4N0's bar and chat (REQUEST21 3).
    pub fn chat(&self) -> &crate::chat_bar::ChatBar {
        &self.chat
    }

    pub fn chat_mut(&mut self) -> &mut crate::chat_bar::ChatBar {
        &mut self.chat
    }

    /// Photos & captures with its capture element open on `part`: where every link of the Capture tab leads now
    /// (REQUEST21 5). Back on its gallery first (a capture's files, an open detail or viewer are left).
    pub fn open_capture(&mut self, part: pages::capture::Part, now: f64) {
        self.set_page(Page::Sessions, now);
        self.sessions.to_gallery();
        self.capture.reveal(part);
    }

    /// The page a name says (a window closed to the tray on it, N4N0's assistant): the Capture tab's old name opens
    /// Photos & captures on its capture element. Whether the name was one.
    fn go_named(&mut self, name: &str, now: f64) -> bool {
        if name == CAPTURE_PAGE {
            self.open_capture(pages::capture::Part::Actions, now);
            return true;
        }
        if name == GROUPS_SECTION {
            self.set_page(Page::Home, now);
            self.home_mut().open_groups();
            return true;
        }
        match page_named(name) {
            Some(page) => {
                self.set_page(page, now);
                true
            }
            None => false,
        }
    }

    /// The capture element of Photos & captures (screenshots stage it).
    pub fn capture_mut(&mut self) -> &mut pages::capture::State {
        &mut self.capture
    }

    /// Settings, on a setting (its registry id, `account.vrchat`) or a sub-tab (`vr`): the sub-tab shows, the setting comes
    /// into view and is lit (`pages::settings::State::open`). An unknown name opens Settings as it was.
    pub fn open_settings(&mut self, target: &str, now: f64) {
        self.set_page(Page::Settings, now);
        self.settings.open(target, now);
    }

    pub fn settings_mut(&mut self) -> &mut pages::settings::State {
        &mut self.settings
    }

    pub fn n4n0_mut(&mut self) -> &mut pages::n4n0::State {
        &mut self.n4n0
    }

    pub fn home_mut(&mut self) -> &mut pages::home::State {
        &mut self.home
    }

    pub fn sessions_mut(&mut self) -> &mut pages::sessions::State {
        &mut self.sessions
    }

    pub fn worlds_mut(&mut self) -> &mut pages::worlds::State {
        &mut self.worlds
    }

    pub fn events_mut(&mut self) -> &mut pages::events::State {
        &mut self.events
    }

    pub fn videos_mut(&mut self) -> &mut pages::videos::State {
        &mut self.videos
    }

    pub fn projects_mut(&mut self) -> &mut pages::projects::State {
        &mut self.projects
    }

    pub(crate) fn stage_vrchat_login(&mut self) {
        self.settings.stage_vrchat(true);
    }

    pub fn toasts_mut(&mut self) -> &mut toast::Toasts {
        &mut self.toasts
    }

    /// Whether the page still reads something it will show (a list, pictures): screenshots wait for it.
    pub fn page_busy(&self) -> bool {
        match self.page {
            Page::Sessions => self.sessions.busy(),
            Page::Worlds => self.worlds.busy(),
            Page::Events => self.events.busy(),
            Page::Videos => self.videos.busy(),
            Page::Projects => self.projects.busy(),
            _ => false,
        }
    }

    /// Notices from the controller become toasts.
    fn drain_notices(&mut self, ctx: &egui::Context) {
        for n in self.ctl.take_notices() {
            // Shown at once, however the window is held (mac_cover.rs).
            #[cfg(target_os = "macos")]
            {
                self.cover.wake();
                ctx.request_repaint();
            }
            match n {
                Notice::Toast { kind, message } => {
                    let kind = match kind {
                        NoticeKind::Info => toast::Kind::Info,
                        NoticeKind::Success => toast::Kind::Success,
                        NoticeKind::Warning => toast::Kind::Warning,
                        NoticeKind::Error => toast::Kind::Error,
                    };
                    self.toasts.push(ctx, kind, message);
                }
                Notice::Retract { messages } => self.toasts.retract(ctx, &messages),
                Notice::SessionSaved { reason, frames, keyframes, depth_frames } => {
                    // A part the recording goes on after (a new world, a new resolution) is no end to tell of.
                    if reason != "restarted" && reason != "resized" {
                        self.toasts.push(
                            ctx,
                            toast::Kind::Success,
                            tr!(
                                "Session saved: {frames} frames, {keyframes} keyframes, {depth_frames} depth frames.",
                                frames = orbiters_ui::format::integer(frames as f64),
                                keyframes = keyframes,
                                depth_frames = depth_frames
                            ),
                        );
                    }
                }
            }
        }
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let snap = self.ctl.view();
        crate::language::follow(&snap.settings);
        {
            let woken = ctx.clone();
            crate::appearance::follow_in_window(&snap.settings, &mut self.wallpaper, move || woken.request_repaint());
        }
        self.follow_size(&ctx, snap.settings.appearance.size.zoom());
        orbiters_ui::follow(&ctx);
        // This process reads Orbiters too (the Map explorer's public pages): from the environment the service uses,
        // production included (else Unity's switch would decide).
        crate::controller::follow_environment(snap.settings.development);
        crate::worlds::profile_pictures::set_cap(u64::from(snap.settings.vrchat_pictures_mb.max(16)) << 20);
        let now = anim::time(&ctx);
        let started = *self.started.get_or_insert(now);
        if self.visit == 0 {
            self.set_page(self.page, now);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        anim::set_live(&ctx, focused && !snap.settings.reduce_motion);

        let full = ui.max_rect();
        // Too small to lay out. (Not how a minimized window is known: no frame is drawn for one at all, only the logic
        // runs, where the pages' watches are told: `pages::watch::follow_window`.)
        if full.width() < 640.0 || full.height() < 400.0 {
            return;
        }
        let mood = match snap.capture.state {
            CaptureState::Importing => Mood::Importing,
            CaptureState::Recording | CaptureState::Paused | CaptureState::Starting => Mood::Recording,
            _ => Mood::Idle,
        };
        if self.wallpaper.covers(&snap.settings.appearance) {
            // Nothing of the glow would be seen; its slow drift asks for no frames either.
            ui.painter().rect_filled(full, 0.0, theme::ink_950());
        } else {
            glow::paint(ui.painter(), full, mood);
        }
        self.paint_background(ui, &snap, full, focused);

        let splash_t = now - started;
        let splash_on = self.show_splash && splash_t < SPLASH + SPLASH_FADE;
        let content_ready = !self.show_splash || splash_t >= SPLASH;

        let title = Rect::from_min_size(full.min, vec2(full.width(), TITLE_H));
        let body = Rect::from_min_max(pos2(full.left(), full.top() + TITLE_H), full.max);
        if !content_ready {
            self.prepare(ui, &snap);
        }
        // The shortcut to N4N0's bar, Esc, and the keys of the page while the bar or the chat has the keyboard.
        if content_ready {
            // What Unity projects' first visit shows at once, read now that the first page is drawn (not beside it).
            self.projects.warm();
            let modal = self.modal_up(&ctx);
            self.chat.before_page(&ctx, &snap, modal);
        }
        if content_ready {
            let since = *self.sidebar_since.get_or_insert(now);
            self.sidebar(ui, &snap, Rect::from_min_size(body.min, vec2(SIDEBAR_W, body.height())), since);
            // From the top of the window: the content scrolls on under the title bar (see `pages::under_top`).
            let page_rect = Rect::from_min_max(pos2(body.left() + SIDEBAR_W, full.top()), body.max);
            self.page(ui, &snap, page_rect);
            let told = self.usage_told || snap.settings.usage.told_all();
            if let Some(answer) = pages::usage_notice::show(ui, page_rect, told) {
                self.usage_told = true;
                self.ctl.patch_settings(serde_json::json!({ "usage": { "told": true, "told_perf": true, "told_account": true } }));
                if answer == pages::usage_notice::Answer::Settings {
                    self.open_settings("privacy.usage", now);
                }
            }
        }
        let strip = self.title_bar(ui, &snap, title);
        // N4N0's bar in the strip, on every page, and the chat it grows into over the page (REQUEST21 3).
        if content_ready {
            self.chat.show(ui, &self.ctl, &snap, strip, self.gpu.as_ref());
        }
        self.toasts.show(&ctx);
        // "Talk with N4N0?": the wake word heard during a call.
        crate::wake_offer::window(&ctx, &self.ctl, snap.n4n0.voice.wake_offer);
        // A friend here offers to look at a 3D picture together (`crate::shared_view_ui`): over that card when both are up.
        let talk_up = crate::wake_offer::up(snap.n4n0.voice.wake_offer, crate::wake_offer::now_ms()).is_some();
        crate::shared_view_ui::window(&ctx, &self.ctl, snap.shared_view.offer.as_ref(), talk_up);
        // "No 3D picture of this world yet" (REQUEST13), over both when they are up, each card 12 above the one under it.
        let shared_up = snap.shared_view.offer.as_ref().is_some_and(|o| o.until_ms > crate::wake_offer::now_ms());
        let lift = if talk_up { crate::wake_offer::HEIGHT + 12.0 } else { 0.0 } + if shared_up { crate::shared_view_ui::HEIGHT + 12.0 } else { 0.0 };
        crate::world_notice::window(&ctx, &self.ctl, snap.explorer.notice.as_ref(), lift);
        // The avatar gun's question or notice (`crate::avatar_gun_ui`), over all of them.
        let notice_up = snap.explorer.notice.as_ref().is_some_and(|n| n.up(crate::controller::explorer::now_ms()));
        let lift = lift + if notice_up { crate::world_notice::CARD.y * 1.1 + 12.0 } else { 0.0 };
        let gun_card = snap.avatar_gun.card.as_ref().filter(|c| c.up(crate::controller::unix_ms()));
        let picture = gun_card.filter(|c| !c.thumbnail.is_empty()).and_then(|c| {
            let scope = crate::controller::vrchat_data::Scope::of(&snap);
            let pictures = self.gun_pictures.get_or_insert_with(|| crate::worlds::Pictures::with_backend(&ctx, Some(self.ctl.clone())));
            pictures.get(&ctx, &crate::worlds::Picture::Avatar { scope, avatar_id: c.avatar_id.clone(), url: c.thumbnail.clone() }).map(|t| t.id())
        });
        crate::avatar_gun_ui::window(&ctx, &self.ctl, gun_card, lift, picture);
        if splash_on {
            splash(&ctx, full, splash_t);
        }
        // The Mac's window has edges of its own (window.rs).
        if self.windowed && cfg!(not(target_os = "macos")) {
            resize_handles(ui, full);
        }
    }

    /// The window's size of everything (Settings, Appearance): egui's zoom, and the smallest window the pages are laid out for at it.
    fn follow_size(&mut self, ctx: &egui::Context, zoom: f32) {
        if (ctx.zoom_factor() - zoom).abs() > 0.001 {
            ctx.set_zoom_factor(zoom);
            if self.windowed {
                // In points, which are as large as the zoom makes them: a larger interface needs a larger window, up to what a small
                // screen has.
                let least = vec2(1040.0, 700.0) * (1.0 / zoom).max(0.93);
                ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(least));
            }
        }
    }

    /// The background chosen, over the glow.
    fn paint_background(&mut self, ui: &Ui, snap: &Snapshot, full: Rect, focused: bool) {
        let a = &snap.settings.appearance;
        let ctx = ui.ctx();
        let motion = crate::appearance::Motion {
            animate: a.animate,
            while_playing: a.animate_while_playing,
            reduced: snap.settings.reduce_motion || (a.background != orbiters_core::appearance::Background::None && crate::appearance::system_reduces_motion()),
            focused: focused && !ctx.input(|i| i.viewport().minimized.unwrap_or(false) || i.viewport().occluded.unwrap_or(false)),
            playing: snap.vrchat.running || snap.steamvr.running,
            resting: anim::is_resting(ctx),
        };
        self.wallpaper.paint(ui.painter(), full, a, motion);
    }

    /// The title strip; says where it put N4N0's bar (drawn after it, with the chat).
    fn title_bar(&mut self, ui: &mut Ui, snap: &Snapshot, rect: Rect) -> crate::chat_bar::layout::Strip {
        let ctx = ui.ctx().clone();
        let id = Id::unique("orbiters-title-bar");
        let bar = ui.interact(rect, id, Sense::click_and_drag());
        if self.windowed {
            if bar.drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
        }
        let p = ui.painter();
        let name = Txt::new("Orbiters - N4N0", 12.0).weight(weight::SEMIBOLD).tracking(0.025).color(theme::fg_2());
        // On a Mac the window's three buttons come first, where every Mac window has them.
        let lead = if cfg!(target_os = "macos") { MAC_BUTTONS_W } else { 16.0 };
        let r = widgets::text(p, pos2(rect.left() + lead, rect.center().y), Align2::LEFT_CENTER, &name);
        // The beta's chip after the app's name, over every page (Enzo, 2026-11-15).
        let r = r.union(beta_chip(ui, pos2(r.right() + 8.0, rect.center().y)));
        // While a recording runs, its Stop from every page (Enzo, F13): never a start.
        let live = !snap.capture.photo_3d && matches!(snap.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Stopping);
        let stopping = snap.capture.state == CaptureState::Stopping;
        let stop = widgets::Button::new(if stopping { tr("Stopping…") } else { tr("Stop recording") }).icon(icons::RECORD).variant(widgets::Variant::Stop).size(widgets::Size::Sm).enabled(!stopping);
        let stop_w = stop.desired_width(ui);
        let stop_at = Rect::from_min_size(pos2(rect.right() - if cfg!(target_os = "macos") { 12.0 } else { 4.0 + 44.0 * 3.0 + 12.0 } - stop_w, rect.top() + 6.0), vec2(stop_w, 28.0));
        // N4N0's bar takes the middle of what is left between the name and the buttons.
        let buttons_start = if cfg!(target_os = "macos") { rect.right() - 12.0 } else { rect.right() - 4.0 - 44.0 * 3.0 };
        let strip = crate::chat_bar::layout::Strip::new(Rect::from_min_max(rect.min, pos2(rect.right(), ui.max_rect().bottom())), TITLE_H, r.right(), if live { stop_at.left() } else { buttons_start });
        let subtitle = match snap.capture.state {
            CaptureState::Recording => Some(tr("Recording").to_string()),
            CaptureState::Importing => Some(tr("Importing").to_string()),
            _ => snap.vrchat.world.world_name.clone(),
        };
        let sub = subtitle.map(|s| format!("·  {s}")).unwrap_or_default();
        widgets::sliding_value(
            ui,
            id.with("subtitle"),
            pos2(r.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            &sub,
            &Txt::new("", 12.0).color(theme::fg_3()),
            // The world's name stops before N4N0's bar.
            (strip.free_until() - r.right() - 8.0).clamp(0.0, 420.0),
        );

        if live && widgets::activated(&stop.show_at(ui, stop_at)) {
            self.ctl.stop_capture();
        }
        // Window buttons, platform order.
        let close = if cfg!(windows) {
            tr("Close (Orbiters keeps running in the tray)")
        } else if cfg!(target_os = "macos") {
            tr("Close (Orbiters keeps running in the menu bar)")
        } else {
            tr("Quit")
        };
        if cfg!(target_os = "macos") {
            self.mac_window_buttons(ui, rect, id, close);
            return strip;
        }
        let buttons = [(icons::MINUS, tr("Minimize"), 15.0, false), (icons::SQUARE, tr("Maximize"), 12.0, false), (icons::X, close, 16.0, true)];
        for (i, (glyph, label, size, danger)) in buttons.into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(rect.right() - 4.0 - 44.0 * (3 - i) as f32, rect.top() + 4.0), vec2(44.0, 32.0));
            let resp = ui.interact(b, id.with(i), Sense::click()).on_hover_text(label);
            let hover = anim::toggle(&ctx, id.with(("hover", i)), resp.hovered(), Spring::SNAPPY);
            let press = anim::toggle(&ctx, id.with(("press", i)), resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let br = Rect::from_center_size(b.center(), b.size() * (1.0 - 0.14 * press));
            let (bg, fg) = if danger {
                (theme::alpha(theme::REC, 0.85 * hover), theme::mix(theme::fg_3(), egui::Color32::WHITE, hover))
            } else {
                (theme::white(0.08 * hover), theme::mix(theme::fg_3(), theme::fg(), hover))
            };
            let p = ui.painter();
            p.rect_filled(br, theme::radius(8.0), bg);
            widgets::icon(p, br.center(), glyph, size, fg);
            if resp.clicked() && self.windowed {
                match i {
                    0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                    1 => {
                        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                    }
                    // The window process ends; the tray service keeps running (and frees the window's memory).
                    _ => ctx.send_viewport_cmd(ViewportCommand::Close),
                }
            }
        }
        strip
    }

    /// A Mac's window buttons, where and as every Mac window has them: close, minimize and full screen as red, yellow
    /// and green dots at the bar's left, their signs shown while the pointer is over the three, grey while the window is
    /// not the front one.
    fn mac_window_buttons(&mut self, ui: &mut Ui, rect: Rect, id: Id, close: &str) {
        let ctx = ui.ctx().clone();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let centre = |i: usize| pos2(rect.left() + 20.0 + 20.0 * i as f32, rect.center().y);
        let group = Rect::from_min_max(centre(0) - vec2(10.0, 10.0), centre(2) + vec2(10.0, 10.0));
        let over = ui.rect_contains_pointer(group);
        let buttons = [(MAC_CLOSE, icons::X, close), (MAC_MINIMIZE, icons::MINUS, tr("Minimize")), (MAC_FULL_SCREEN, icons::ARROWS_OUT, tr("Full screen"))];
        for (i, (colour, glyph, label)) in buttons.into_iter().enumerate() {
            let c = centre(i);
            let resp = ui.interact(Rect::from_center_size(c, vec2(18.0, 18.0)), id.with(("mac", i)), Sense::click()).on_hover_text(label);
            let down = resp.is_pointer_button_down_on();
            let p = ui.painter();
            let fill = if focused || over { colour } else { theme::ink_600() };
            p.circle_filled(c, 6.0, if down { theme::mix(fill, egui::Color32::BLACK, 0.25) } else { fill });
            p.circle_stroke(c, 6.0, egui::Stroke::new(0.5, theme::black(0.2)));
            if over {
                widgets::icon(p, c, glyph, 8.5, theme::black(0.6));
            }
            if resp.clicked() && self.windowed {
                match i {
                    // The window process ends; the menu bar's service keeps running (and frees the window's memory).
                    0 => ctx.send_viewport_cmd(ViewportCommand::Close),
                    1 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                    _ => {
                        let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!full));
                    }
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, snap: &Snapshot, rect: Rect, since: f64) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let id = Id::unique("orbiters-sidebar");
        let inner = rect.shrink2(vec2(16.0, 0.0));

        // The N4N0 mark and the app's name; hovering replays the signal glitch.
        let logo_rect = Rect::from_min_size(pos2(inner.left() + 8.0, inner.top() + 8.0), n4n0_logo::size_for_width(36.0));
        let row = Rect::from_min_size(logo_rect.min, vec2(150.0, logo_rect.height()));
        let hovered = ui.interact(row, id.with("logo"), Sense::hover()).hovered();
        if hovered && !self.logo_hovered {
            self.logo_glitch = Some((now, 0.8));
        }
        self.logo_hovered = hovered;
        let intro = self.sidebar_intro(now);
        n4n0_logo::paint_glitched(ui.painter(), logo_rect, self.logo_glitch.or(intro));
        let p = ui.painter();
        widgets::text(
            p,
            pos2(logo_rect.right() + 12.0, logo_rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            &Txt::new("N4N0", 17.0).weight(weight::SEMIBOLD).tracking(0.02),
        );
        widgets::text(p, pos2(logo_rect.right() + 12.0, logo_rect.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new("Orbiters", 11.5).color(theme::fg_3()));
        let name_wide = widgets::text_size(ui, &Txt::new("N4N0", 17.0).weight(weight::SEMIBOLD).tracking(0.02)).x.max(widgets::text_size(ui, &Txt::new("Orbiters", 11.5)).x);
        self.update_button(ui, snap, pos2(inner.right(), logo_rect.center().y), logo_rect.right() + 12.0 + name_wide + 8.0, id.with("update"));

        // Navigation with a pill that slides to the active page.
        let nav_top = logo_rect.bottom() + 28.0;
        let items = [
            (Page::Home, tr("Home"), icons::HOUSE),
            (Page::N4n0, "N4N0", icons::N4N0),
            (Page::Projects, tr("Unity projects"), icons::CUBE),
            (Page::Sessions, tr("Photos & captures"), icons::IMAGES),
            (Page::Worlds, tr("Visited worlds"), icons::GLOBE),
            (Page::Events, tr("Events"), icons::CALENDAR_DOTS),
            (Page::Videos, tr("Video history"), icons::FILM_STRIP),
            (Page::Settings, tr("Settings"), icons::SLIDERS_HORIZONTAL),
        ];
        let item_rect = |i: usize| Rect::from_min_size(pos2(inner.left(), nav_top + i as f32 * 48.0), vec2(inner.width(), 44.0));
        let active = items.iter().position(|(pg, _, _)| *pg == self.page).unwrap_or(0);
        let pill_y = anim::value(&ctx, id.with("pill"), (item_rect(active).top() - nav_top) as f32, Spring::BOUNCY);
        let pill = Rect::from_min_size(pos2(inner.left(), nav_top + pill_y), vec2(inner.width(), 44.0));
        let p = ui.painter();
        p.rect_filled(pill, theme::radius(14.0), theme::white(0.07));
        let bar = Rect::from_center_size(pos2(pill.left() + 1.5, pill.center().y), vec2(3.0, 20.0));
        p.rect_filled(bar, theme::radius(1.5), theme::accent());

        let capturing = snap.is_capturing();
        for (i, (page, label, glyph)) in items.into_iter().enumerate() {
            let (alpha, dx) = nav_entrance(ui, id.with(("enter", i)), since, i);
            let r = item_rect(i).translate(vec2(dx, 0.0));
            let resp = ui.interact(item_rect(i), id.with(("item", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if widgets::pressed(&resp) {
                self.set_page(page, now);
            }
            let is_active = page == self.page;
            let press = anim::toggle(&ctx, id.with(("press", i)), resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let target = if is_active {
                egui::Color32::WHITE
            } else if resp.hovered() {
                theme::fg_2()
            } else {
                theme::fg_3()
            };
            let fg = anim::color(&ctx, id.with(("fg", i)), target, Spring::SNAPPY);
            let icon_c = anim::color(&ctx, id.with(("ic", i)), if is_active { theme::accent() } else { target }, Spring::SNAPPY);
            let mut p = ui.painter().clone();
            p.multiply_opacity(alpha);
            let x = r.left() + 14.0 + 2.0 * press;
            if is_active {
                widgets::icon_fill(&p, pos2(x + 9.0, r.center().y), glyph, 18.0, icon_c);
            } else {
                widgets::icon(&p, pos2(x + 9.0, r.center().y), glyph, 18.0, icon_c);
            }
            widgets::text(&p, pos2(x + 30.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(label, 14.0).weight(weight::MEDIUM).color(fg));
            // Photos & captures: how many captures it holds, and on its icon the dot of one under way (the Capture tab's,
            // which it took in).
            if page == Page::Sessions && capturing {
                let tone = if snap.capture.state == CaptureState::Importing { widgets::Tone::Info } else { widgets::Tone::Rec };
                let at = pos2(x + 18.0, r.center().y - 8.0);
                p.circle_filled(at, 5.5, theme::ink_950());
                widgets::status_dot(&p, at, tone, true);
            }
            if page == Page::Sessions && snap.storage.sessions > 0 {
                let t = Txt::new(snap.storage.sessions.to_string(), 11.0).color(theme::fg_3());
                let s = widgets::text_size(ui, &t);
                let badge = Rect::from_min_size(pos2(r.right() - 12.0 - s.x - 16.0, r.center().y - 10.0), vec2(s.x + 16.0, 20.0));
                p.rect_filled(badge, theme::radius(10.0), theme::white(0.07));
                widgets::text(&p, badge.center(), Align2::CENTER_CENTER, &t);
            }
        }

        // Status at the bottom: the version, and the development environment when it is the one in use.
        let bottom = inner.bottom() - 16.0;
        let version = if snap.settings.development { tr!("v{version} · development", version = snap.version) } else { format!("v{}", snap.version) };
        let version = Txt::new(version, 11.0).color(if snap.settings.development { theme::WARN } else { theme::fg_4() });
        widgets::text(ui.painter(), pos2(inner.left() + 8.0, bottom - 6.0), Align2::LEFT_CENTER, &version);
        let osc = &snap.osc;
        let osc_tone = if osc.listening {
            if osc.advertising {
                widgets::Tone::Ok
            } else {
                widgets::Tone::Warn
            }
        } else {
            widgets::Tone::Off
        };
        let osc_text = if osc.listening {
            match osc.port {
                Some(port) => tr!("OSC ready · {port}", port = port),
                None => tr("OSC ready").into(),
            }
        } else if osc.enabled {
            tr("OSC unavailable").into()
        } else {
            tr("OSC off").into()
        };
        let (vrchat_text, vrchat_tone) = if orbiters_core::osc::VRCHAT_RUNS_HERE {
            let text = if snap.vrchat.running { snap.vrchat.world.world_name.clone().unwrap_or_else(|| tr("Running").into()) } else { tr("Not running").into() };
            (text, if snap.vrchat.running { widgets::Tone::Ok } else { widgets::Tone::Off })
        } else {
            pages::network::sidebar_status(&snap.lan)
        };
        side_status(ui, pos2(inner.left(), bottom - 34.0 - 44.0), tr("Action Menu"), &osc_text, osc_tone, inner.width());
        // Several VRChats on this computer: how many, and the one pictures are taken in (its account, its world); a press
        // opens Photos & captures' capture element on them, where another is chosen.
        let several = snap.vrchat.several_target().filter(|_| orbiters_core::osc::VRCHAT_RUNS_HERE);
        let (vrchat_label, vrchat_text) = match several {
            Some(target) => {
                let who = crate::controller::vrchats::who(target);
                let text = match &target.world {
                    Some(world) => format!("{who} · {world}"),
                    None => who,
                };
                (tr_n("{count} VRChat", "{count} VRChats", snap.vrchat.runs.len() as u64), text)
            }
            None => ("VRChat".to_string(), vrchat_text),
        };
        let vrchat_row = Rect::from_min_size(pos2(inner.left(), bottom - 34.0 - 88.0), vec2(inner.width(), 40.0));
        if several.is_some() {
            let resp = ui.interact(vrchat_row, Id::unique("side-vrchats"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if resp.hovered() {
                ui.painter().rect_filled(vrchat_row.shrink2(vec2(4.0, 0.0)), theme::radius(10.0), theme::white(0.04));
            }
            if widgets::pressed(&resp) {
                self.open_capture(pages::capture::Part::Vrchats, now);
            }
        }
        side_status(ui, vrchat_row.min, &vrchat_label, &vrchat_text, vrchat_tone, inner.width());
        let (account_text, account_tone) = match &snap.account.state {
            AccountState::SignedIn { username } => (username.clone(), widgets::Tone::Ok),
            AccountState::Checking { username } | AccountState::Offline { username } => (username.clone().unwrap_or_default(), widgets::Tone::Warn),
            AccountState::Expired { .. } => (tr("Sign-in expired").into(), widgets::Tone::Warn),
            AccountState::SigningIn => (tr("Finishing in your browser").into(), widgets::Tone::Warn),
            AccountState::SignedOut | AccountState::Unknown => (tr("Not signed in").into(), widgets::Tone::Off),
        };
        side_status(ui, pos2(inner.left(), bottom - 34.0 - 132.0), "Orbiters", &account_text, account_tone, inner.width());
    }

    /// The Update button beside the name (its right edge at `right_center`), as soon as a newer version is found: a
    /// progress ring while it downloads, then the green button that restarts into it (or, while a recording or another
    /// job would be cut short, what it waits for). Nothing while the app is up to date.
    fn update_button(&mut self, ui: &mut Ui, snap: &Snapshot, right_center: egui::Pos2, name_end: f32, id: Id) {
        let ctx = ui.ctx().clone();
        let u = &snap.update;
        let (label, tip, fraction, live) = match &u.state {
            UpdateState::Downloading { version, fraction } => {
                (tr("Update"), tr!("Downloading version {version}: {percent} %", version = version, percent = format!("{:.0}", fraction * 100.0)), Some(*fraction), false)
            }
            UpdateState::Ready { version, .. } if u.restart_when_idle => {
                let work = tr(snap.busy_with().unwrap_or(mark("the running work")));
                (tr("Waiting"), tr!("Version {version} installs once {work} ends (Settings, About)", version = version, work = work), None, false)
            }
            UpdateState::Ready { version, notes } => {
                let first = notes.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| format!("\n{l}")).unwrap_or_default();
                let tip = match snap.busy_with() {
                    Some(work) => tr!("Version {version} is ready: it installs once {work} ends{first}", version = version, work = tr(work), first = first),
                    None => tr!("Version {version} is ready: Orbiters - N4N0 restarts to install it{first}", version = version, first = first),
                };
                (tr("Update"), tip, None, true)
            }
            _ => return,
        };
        let words = |size: f32| Txt::new(label, size).weight(weight::SEMIBOLD).color(if live { theme::on_accent() } else { theme::accent() });
        // Beside the name, in the room it leaves: a longer word ("Mettre à jour") first gives up the icon (a download keeps its ring, which is its progress), then a little size.
        let (room, wide) = (right_center.x - name_end, widgets::text_size(ui, &words(11.5)).x);
        let with_icon = fraction.is_some() || wide + 34.0 <= room;
        let lead = if with_icon { 23.0 } else { 11.0 };
        let size = if lead + wide + 11.0 > room { (11.5 * (room - lead - 11.0) / wide).max(10.0) } else { 11.5 };
        let txt = words(size);
        let w = widgets::text_size(ui, &txt).x + lead + 11.0;
        let pill = Rect::from_min_size(pos2(right_center.x - w, right_center.y - 13.0), vec2(w, 26.0));
        let resp = ui.interact(pill, id, Sense::click()).on_hover_text(tip);
        let resp = if live { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        let hover = anim::toggle(&ctx, id.with("hover"), live && resp.hovered(), Spring::SNAPPY);
        let press = anim::toggle(&ctx, id.with("press"), live && resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let shown = Rect::from_center_size(pill.center(), pill.size() * (1.0 - 0.06 * press));
        let p = ui.painter();
        let fill = if live { theme::mix(theme::accent(), theme::accent_hover(), hover) } else { theme::alpha(theme::accent(), 0.14) };
        p.rect_filled(shown, theme::radius(13.0), fill);
        let at = pos2(shown.left() + 13.0, shown.center().y);
        match fraction {
            Some(f) => {
                orbiters_ui::paint::ring(p, at, 5.0, 2.0, theme::alpha(theme::accent(), 0.3));
                orbiters_ui::paint::arc(p, at, 5.0, -std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * f.clamp(0.0, 1.0), 2.0, theme::accent());
            }
            None if with_icon => widgets::icon(p, at, icons::DOWNLOAD_SIMPLE, 12.5, if live { theme::on_accent() } else { theme::accent() }),
            None => {}
        }
        widgets::text(p, pos2(shown.left() + lead, shown.center().y), Align2::LEFT_CENTER, &txt);
        if live && widgets::pressed(&resp) {
            self.ctl.updates(UpdateAction::Restart { now: false });
        }
    }

    /// The intro glitch of the sidebar logo, once, right after the splash.
    fn sidebar_intro(&self, now: f64) -> Option<(f64, f32)> {
        let since = self.sidebar_since?;
        (now - since < 1.2).then_some((since + 0.15, 1.0))
    }

    /// Where the person is, for the usage statistics: told when it changed (a comparison a frame, a name only then).
    fn note_screen(&mut self) {
        let here = (self.page, (self.page == Page::Home).then(|| self.home.shown()).flatten());
        if self.usage_here != Some(here) {
            self.usage_here = Some(here);
            let page = format!("{:?}", here.0);
            let feature = here.1.map(|p| crate::usage::stable_name(&format!("{p:?}"))).unwrap_or_default();
            crate::usage::enter(&crate::usage::screen_name([page.into(), feature.into()]));
        }
    }

    /// Under the splash: what the page about to show asks for at its first frame is asked now, so that it is there (or
    /// nearly) when the page is. Nothing is drawn: a page's entrances are to be seen.
    fn prepare(&mut self, ui: &Ui, snap: &Arc<Snapshot>) {
        let mut cx = pages::Cx {
            ctl: &self.ctl,
            snap,
            toasts: &mut self.toasts,
            entered: self.page_since,
            visit: self.visit,
            windowed: self.windowed,
            gpu: self.gpu.as_ref(),
            top: TITLE_H,
            go: None,
        };
        match self.page {
            Page::Home => pages::home::prepare(ui, &mut cx, &mut self.home),
            Page::Projects => pages::projects::prepare(ui, &cx, &mut self.projects),
            Page::Sessions => pages::sessions::prepare(ui, &cx, &mut self.sessions),
            Page::Worlds => pages::worlds::prepare(ui, &cx, &mut self.worlds),
            Page::Events => pages::events::prepare(ui, &cx, &mut self.events),
            Page::Videos => pages::videos::prepare(ui, &cx, &mut self.videos),
            Page::N4n0 | Page::Settings => {}
        }
    }

    fn page(&mut self, ui: &mut Ui, snap: &Arc<Snapshot>, rect: Rect) {
        let ctx = ui.ctx().clone();
        self.note_screen();
        // Enter: slide in from the right and fade up with a soft spring.
        let v = anim::spring_from(&ctx, Id::unique(("orbiters-page", self.visit)), [0.0], [1.0], Spring::SOFT)[0];
        let shifted = rect.translate(vec2(24.0 * (1.0 - v), 0.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(shifted).id_salt(("page", self.visit)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child.multiply_opacity(v.clamp(0.0, 1.0));
        let mut cx = pages::Cx {
            ctl: &self.ctl,
            snap,
            toasts: &mut self.toasts,
            entered: self.page_since,
            visit: self.visit,
            windowed: self.windowed,
            gpu: self.gpu.as_ref(),
            top: TITLE_H,
            go: None,
        };
        // An editor being installed ends by opening its project, whatever page shows.
        self.projects.poll_opening(&ctx, &mut cx);
        match self.page {
            Page::Home => pages::home::show(&mut child, &mut cx, &mut self.home),
            Page::Projects => pages::projects::show(&mut child, &mut cx, &mut self.projects),
            Page::N4n0 => pages::n4n0::show(&mut child, &mut cx, &mut self.n4n0),
            Page::Sessions => pages::sessions::show(&mut child, &mut cx, &mut self.sessions, &mut self.capture, &mut self.projects),
            Page::Worlds => pages::worlds::show(&mut child, &mut cx, &mut self.worlds),
            Page::Events => pages::events::show(&mut child, &mut cx, &mut self.events),
            Page::Videos => pages::videos::show(&mut child, &mut cx, &mut self.videos),
            Page::Settings => pages::settings::show(&mut child, &mut cx, &mut self.settings),
        }
        if let Some(go) = cx.go {
            match go {
                pages::Go::FreeSpace => {
                    self.set_page(Page::Sessions, anim::time(&ctx));
                    self.sessions.open_storage(false);
                }
                pages::Go::Projects => self.set_page(Page::Projects, anim::time(&ctx)),
                go => {
                    if let Some(target) = go.setting() {
                        self.open_settings(target, anim::time(&ctx));
                    }
                }
            }
        }
    }
}

/// The chip that says this is a beta, its left end at `left_center`: the accent on a tint of it, as the website's badges are.
pub(crate) fn beta_chip(ui: &Ui, left_center: egui::Pos2) -> Rect {
    let word = orbiters_i18n::tr_ctx("chip", "Beta").to_uppercase();
    let txt = Txt::new(word, 10.0).weight(weight::BOLD).tracking(0.06).color(theme::accent());
    let size = widgets::text_size(ui, &txt);
    let chip = Rect::from_min_size(pos2(left_center.x, left_center.y - 9.0), vec2(size.x + 14.0, 18.0));
    let p = ui.painter();
    p.rect_filled(chip, theme::radius(9.0), theme::alpha(theme::accent(), 0.16));
    widgets::text(p, chip.center(), Align2::CENTER_CENTER, &txt);
    chip
}

fn nav_entrance(ui: &Ui, id: Id, since: f64, i: usize) -> (f32, f32) {
    let ctx = ui.ctx();
    let delay = 0.15 + i as f64 * 0.06;
    let elapsed = anim::time(ctx) - since;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let v = anim::spring_from(ctx, id, [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
    (v.clamp(0.0, 1.0), -12.0 * (1.0 - v))
}

fn side_status(ui: &Ui, top_left: egui::Pos2, label: &str, text: &str, tone: widgets::Tone, width: f32) {
    let p = ui.painter();
    let row = Rect::from_min_size(top_left, vec2(width, 40.0));
    widgets::status_dot(p, pos2(row.left() + 15.0, row.center().y), tone, tone == widgets::Tone::Ok);
    widgets::text(p, pos2(row.left() + 30.0, row.center().y - 8.0), Align2::LEFT_CENTER, &Txt::caps(label));
    widgets::text_truncated(p, pos2(row.left() + 30.0, row.center().y + 8.0), Align2::LEFT_CENTER, &Txt::new(text, 12.0).color(theme::fg_2()), width - 40.0);
}

/// Opening moment: the mark draws itself in with its signal echoes, the name rises, then everything lifts away.
fn splash(ctx: &egui::Context, full: Rect, t: f64) {
    let fade = if t > SPLASH { 1.0 - ((t - SPLASH) / SPLASH_FADE) as f32 } else { 1.0 };
    let fade = ease::out_cubic(fade.clamp(0.0, 1.0));
    if t < SPLASH + SPLASH_FADE {
        ctx.request_repaint();
    }
    orbiters_ui::front::area(ctx, Id::unique("orbiters-splash"), egui::Order::Foreground, true).fixed_pos(full.min).interactable(true).show(ctx, |ui| {
        let (_, _) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
        let mut p = ui.painter().clone();
        p.multiply_opacity(fade);
        p.rect_filled(full, 0.0, theme::ink_950());
        let start = anim::time(ctx) - t;
        let scale_k = anim::spring_from(ctx, Id::unique("orbiters-splash-scale"), [0.82], [1.0], Spring { stiffness: 180.0, damping: 18.0, mass: 1.0 })[0];
        let lift = 1.0 + 0.04 * (1.0 - fade);
        let center = full.center() - vec2(0.0, 36.0);
        // Rasterised at its resting size and scaled as it grows: one set of textures, not one per frame's size.
        let rest = Rect::from_center_size(center, n4n0_logo::size_for_width(128.0));
        let logo_rect = Rect::from_center_size(center, rest.size() * scale_k * lift);
        let mut lp = p.clone();
        lp.multiply_opacity(((t / 0.35) as f32).clamp(0.0, 1.0));
        n4n0_logo::paint_zoomed(&lp, rest, scale_k * lift, Some((start + 0.3, 1.2)));
        let k = ease::segment(t as f32, 0.75, 0.6, ease::out_expo);
        let mut tp = p.clone();
        tp.multiply_opacity(k);
        // The mark spells N4N0: under it, only who makes it.
        let y = logo_rect.bottom() + 24.0 + 14.0 * (1.0 - k);
        let galley = tp.layout_job(Txt::new("ORBITERS", 13.0).weight(weight::MEDIUM).tracking(0.32).color(theme::fg_3()).job(f32::INFINITY));
        let at = pos2(full.center().x - galley.size().x * 0.5 + 0.16 * 13.0, y + 6.0 - galley.size().y * 0.5);
        tp.galley(at, galley, theme::fg());
    });
}

/// Invisible borders that resize the frameless window.
fn resize_handles(ui: &mut Ui, full: Rect) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let e = 5.0;
    let c = 12.0;
    let zones = [
        (Rect::from_min_size(full.min, Vec2::splat(c)), ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
        (Rect::from_min_size(pos2(full.right() - c, full.top()), Vec2::splat(c)), ResizeDirection::NorthEast, CursorIcon::ResizeNeSw),
        (Rect::from_min_size(pos2(full.left(), full.bottom() - c), Vec2::splat(c)), ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (Rect::from_min_size(full.max - Vec2::splat(c), Vec2::splat(c)), ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
        (Rect::from_min_max(pos2(full.left() + c, full.top()), pos2(full.right() - c, full.top() + e)), ResizeDirection::North, CursorIcon::ResizeVertical),
        (
            Rect::from_min_max(pos2(full.left() + c, full.bottom() - e), pos2(full.right() - c, full.bottom())),
            ResizeDirection::South,
            CursorIcon::ResizeVertical,
        ),
        (Rect::from_min_max(pos2(full.left(), full.top() + c), pos2(full.left() + e, full.bottom() - c)), ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (
            Rect::from_min_max(pos2(full.right() - e, full.top() + c), pos2(full.right(), full.bottom() - c)),
            ResizeDirection::East,
            CursorIcon::ResizeHorizontal,
        ),
    ];
    for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui.interact(rect, Id::unique(("orbiters-resize", i)), Sense::drag()).on_hover_cursor(cursor);
        if resp.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

/// Windows 11 rounds the corners of a frameless window only when asked.
#[cfg(windows)]
fn round_corners(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
    let Ok(handle) = cc.window_handle() else { return };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let hwnd = HWND(h.hwnd.get() as *mut _);
        let pref = DWMWCP_ROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, std::mem::size_of_val(&pref) as u32);
        }
    }
}

/// The window's icon (taskbar, Alt+Tab).
pub fn window_icon() -> egui::IconData {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/128x128@2x.png")));
    let Ok(mut reader) = decoder.read_info() else { return egui::IconData::default() };
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let Ok(info) = reader.next_frame(&mut buf) else { return egui::IconData::default() };
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return egui::IconData::default(),
    };
    egui::IconData { rgba, width: info.width, height: info.height }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(not(target_os = "macos"))]
        let _ = frame;
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        // A Mac's window that macOS called covered is not drawn (eframe skips it), and macOS does not always say when
        // it shows again (mac_cover.rs).
        #[cfg(target_os = "macos")]
        let game_in_front = self.windowed && self.follow_cover(ctx, focused, crate::mac_cover::native(frame));
        #[cfg(not(target_os = "macos"))]
        let game_in_front = false;
        if self.ctl.take_show() {
            bring_to_front(ctx);
        }
        // Asked from the menus around VRChat: My Avatar, to add the capture kit, is on Projects.
        if let Some(page) = self.ctl.take_show_page() {
            if page == "chat" {
                // SteamVR's dashboard, where a question N4N0 asks cannot be answered.
                self.chat.open();
                bring_to_front(ctx);
            } else if page == "projects" {
                self.set_page(Page::Projects, anim::time(ctx));
            } else if page == "vrchat-account" {
                // Previous instances' Connect VRChat in the menus around VRChat (REQUEST20 4): where Home's Sign in to VRChat leads.
                self.open_settings("account.vrchat", anim::time(ctx));
                bring_to_front(ctx);
            } else if let Some(target) = page.strip_prefix("settings:") {
                // A setting of the window's Settings, by its registry id or its sub-tab's (REQUEST21 2).
                self.open_settings(target, anim::time(ctx));
                bring_to_front(ctx);
            } else if let Some(dir) = page.strip_prefix("share-to:") {
                // "Share to …" beside VRChat's camera on the desktop: this window's own share dialog, on its friends.
                let server = if self.ctl.view().settings.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                if let Some(entry) = crate::controller::shares::session_entry(std::path::Path::new(dir), server) {
                    self.set_page(Page::Sessions, anim::time(ctx));
                    self.sessions.gallery_mut().open_share_to(ctx, entry);
                    bring_to_front(ctx);
                }
            } else if let Some(name) = crate::controller::assistant::window_page(&page) {
                // N4N0's assistant opened one of the window's pages (REQUEST20 6).
                if self.go_named(name, anim::time(ctx)) {
                    bring_to_front(ctx);
                }
            } else if let Some(dir) = page.strip_prefix("session:") {
                // Asked from a capture turning on a sheet: it opens here, in the app's own 3D viewer.
                self.set_page(Page::Sessions, anim::time(ctx));
                let middle = ctx.content_rect().center();
                self.sessions.open_session(ctx, std::path::PathBuf::from(dir), Rect::from_center_size(middle, vec2(300.0, 200.0)));
            }
        }
        self.ctl.set_background(!focused && (game_in_front || self.ctl.view().vrchat.runs_here()));
        // Minimized, no frame is drawn and only this runs: what the pages watch on their threads stops until the window
        // shows again (the microphones Settings listens to, Unity's editors, the folders).
        #[cfg(target_os = "macos")]
        let shown = self.cover.visible().filter(|_| self.windowed);
        #[cfg(not(target_os = "macos"))]
        let shown = None;
        pages::watch::follow_window_as(ctx, shown);
        self.follow_rest(ctx);
        if self.ctl.should_close() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.drain_notices(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
        orbiters_ui::autoscroll::paint(ui.ctx());
    }

    /// A drag out of the window asked for in the last frame (Photos & captures' cards) runs here, once that frame is on
    /// screen: Windows' drag loop is modal (`drag_out`).
    /// Then the mouse wheel's button scrolls what is under it, as in browsers (`orbiters_ui::autoscroll`).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::drag_out::run_pending(ctx, raw_input);
        orbiters_ui::autoscroll::raw_input(ctx, raw_input);
    }

    /// The window closes with Talk held: let go of it, or VRChat would stay muted and N4N0 go on listening.
    fn on_exit(&mut self) {
        self.n4n0.leave(&self.ctl);
        self.chat.leave(&self.ctl);
        crate::usage::flush(&self.ctl);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::ink_950().to_normalized_gamma_f32()
    }
}

impl App {
    /// What must ALL hold for the window to close itself to the tray (`crate::window_rest`): safe by construction, the
    /// window closes only on an explicit list of idle facts, and any work of the window counts (every page `Job` and
    /// the few other threads that write: `pages::Working`), so a job added later blocks it without anyone listing it
    /// (SWEEP-18: a project copy, a Library move, a package update, a Toolkit install were cut by a list of busy
    /// things that did not name them).
    pub(crate) fn idle_facts(&self, ctx: &egui::Context, snap: &Snapshot) -> [(&'static str, bool); 14] {
        use crate::state::UploadState;
        [
            ("no work of the window runs (a job, a copy, a move, an install, a save, a scan)", pages::working_now() == 0), // i18n: not shown (a reason in code, never displayed)
            ("no upload prepares or sends", !snap.uploads.iter().any(|u| matches!(u.state, UploadState::Preparing | UploadState::Sending))), // i18n: not shown (a reason in code, never displayed)
            ("no reconstruction runs", snap.reconstruction.is_none()), // i18n: not shown (a reason in code, never displayed)
            ("nothing records or imports", !snap.is_capturing()), // i18n: not shown (a reason in code, never displayed)
            ("no clean-up runs", snap.cleanup.is_none()), // i18n: not shown (a reason in code, never displayed)
            ("no share is under way", !snap.capture_shares.iter().any(|j| j.outcome.is_none())), // i18n: not shown (a reason in code, never displayed)
            ("no call", snap.n4n0.calls.call.is_none()), // i18n: not shown (a reason in code, never displayed)
            ("no menu is open", !orbiters_ui::popup::any_open(ctx)), // i18n: not shown (a reason in code, never displayed)
            ("nothing is open on Home (a profile, the music page)", !self.home.holds()), // i18n: not shown (a reason in code, never displayed)
            ("nothing is open in Photos & captures or the captures (a picture, an editor, a viewer, a dialog)", !self.sessions.holds()), // i18n: not shown (a reason in code, never displayed)
            ("no world's page or Share window is open", !self.worlds.holds()), // i18n: not shown (a reason in code, never displayed)
            ("no event's or picture's window is open", !self.events.holds()), // i18n: not shown (a reason in code, never displayed)
            ("no dialog is open on Unity projects", !self.projects.holds()), // i18n: not shown (a reason in code, never displayed)
            // N4N0's chat up, something typed in it, Talk held: closing would lose them (verify-r21 8).
            ("N4N0's chat is closed, nothing is typed in it, Talk is not held", !self.chat.holds()), // i18n: not shown (a reason in code, never displayed)
        ]
    }

    /// Whether the window may rest to the tray as far as its own work goes (every fact of [`App::idle_facts`]).
    pub(crate) fn may_rest(&self, ctx: &egui::Context, snap: &Snapshot) -> bool {
        self.idle_facts(ctx, snap).iter().all(|(_, idle)| *idle)
    }

    /// Something of the window's holds the foreground (a sheet, a dialog, a viewer, a list, the first-use notice): the
    /// keyboard and the pointer are its own, and N4N0's chat stands aside (verify-r21 major 7). Each says so itself as it
    /// is shown (`orbiters_ui::front`, verify2-r21 M5); what a page holds in its own layer is added at once.
    pub(crate) fn modal_up(&self, ctx: &egui::Context) -> bool {
        let page = match self.page {
            Page::Home => self.home.modal_up(),
            Page::Sessions => self.sessions.holds(),
            Page::Worlds => self.worlds.holds(),
            Page::Events => self.events.holds(),
            Page::Projects => self.projects.holds(),
            _ => false,
        };
        page || orbiters_ui::front::held(ctx)
    }

    /// A Mac's window held hidden: drawn only when it shows after all, or has something to show. Whether a game of the engine
    /// is in front.
    #[cfg(target_os = "macos")]
    fn follow_cover(&mut self, ctx: &egui::Context, focused: bool, native: crate::mac_cover::Native) -> bool {
        use crate::mac_cover::{look, woken, Step, LOOK_GAME};
        let told = Told::of(&self.ctl.view());
        if woken() || self.told.replace(told.clone()).is_some_and(|before| before != told) {
            self.cover.wake();
        }
        let (said, hidden) = ctx.input(|i| (i.viewport().minimized == Some(true), i.viewport().occluded == Some(true)));
        // eframe asks macOS once, as the window is made, and never again: it is told.
        let (minimized, correct) = self.cover.minimized(said, native.minimized);
        if let Some(is) = correct {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(is));
        }
        if minimized {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(LOOK_GAME));
            return false;
        }
        match self.cover.step(self.rest_clock.elapsed().as_secs_f64(), hidden, focused, || look(native.id)) {
            Step::AsAsked => {}
            Step::Draw(again) => {
                // (asking for it asks for a frame now: only ever when this one is to be drawn)
                ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(again));
            }
            Step::Wait(again) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(again)),
        }
        self.cover.game()
    }

    /// One look at the countdown to closing to the tray: the offer up, taken down, or the window closing (as its
    /// close button does, its page kept for the next window). Looked at again when the countdown needs it.
    fn follow_rest(&mut self, ctx: &egui::Context) {
        use crate::window_rest::{Facts, Step, Where};
        if !self.windowed {
            return;
        }
        let snap = self.ctl.view();
        #[cfg(windows)]
        let desktop = crate::window_rest::card::can_show();
        #[cfg(not(windows))]
        let desktop = false;
        let now = self.rest_clock.elapsed().as_secs_f64();
        let now_ms = crate::wake_offer::now_ms();
        // What the desktop card confirms on the screen, and a Keep open pressed on it: read right before the look, so
        // that a press arriving before the close happens wins (verify2).
        #[cfg(windows)]
        let (card_shows, kept_on_desktop) = (self.rest.offer(now, now_ms).is_some_and(|(o, _)| self.rest_card.shows() == Some(o.id)), self.rest_card.take_keep());
        #[cfg(not(windows))]
        let (card_shows, kept_on_desktop) = (false, false);
        let facts = Facts {
            minimized: ctx.input(|i| i.viewport().minimized.unwrap_or(false)),
            vrchat: snap.vrchat.running && snap.vrchat.runs_here(),
            seen_at: crate::window_rest::where_seen(snap.steamvr.running, snap.steamvr.overlay, snap.steamvr.headset_in_use, desktop),
            working: !self.idle_facts(ctx, &snap).iter().all(|(_, idle)| *idle),
            on: snap.settings.close_minimized_window,
            shown_in_vr: snap.window_rest.shown,
            kept_in_vr: snap.window_rest.kept,
            card_shows,
            kept_on_desktop,
        };
        match self.rest.step(now, now_ms, &facts) {
            Step::Show(offer, Where::Vr) => self.ctl.window_rest(Some(offer)),
            Step::Show(_, Where::Desktop) | Step::Nothing => {}
            Step::Withdraw => {
                self.ctl.window_rest(None);
                #[cfg(windows)]
                self.rest_card.hide();
            }
            Step::Close => {
                #[cfg(windows)]
                self.rest_card.hide();
                self.ctl.window_rest(None);
                crate::window_rest::resume::save(&format!("{:?}", self.page));
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }
        // The desktop card shows its offer wherever a card can be seen (its countdown runs once Windows confirmed it);
        // where it cannot (a game in exclusive full screen), it is not drawn and the countdown pauses.
        #[cfg(windows)]
        match self.rest.offer(now, now_ms) {
            Some((offer, Where::Desktop)) if facts.seen_at == Some(Where::Desktop) => {
                self.rest_card.present(offer.id, ctx.pixels_per_point(), offer.left(now_ms), offer.seconds(now_ms));
            }
            _ => self.rest_card.hide(),
        }
        if let Some(t) = self.rest.next_look(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(t));
        }
    }
}

/// What the window shows of the service's state whatever its page (its side bar, Home): a change of it is drawn at once even
/// where the window is held hidden (mac_cover.rs). Nothing in it changes but when something happened.
#[cfg(target_os = "macos")]
#[derive(Clone)]
struct Told {
    vrchat: (bool, Option<String>, Option<String>),
    launcher: Arc<orbiters_core::launcher::LauncherState>,
    account: Arc<crate::state::AccountStatus>,
    settings: Arc<orbiters_core::settings::Settings>,
    capture: CaptureState,
    update: std::mem::Discriminant<UpdateState>,
    osc: (bool, bool, Option<u16>),
    lan: (std::mem::Discriminant<crate::state::LanLink>, Option<String>),
    call: bool,
}

#[cfg(target_os = "macos")]
impl Told {
    fn of(snap: &Snapshot) -> Told {
        Told {
            vrchat: (snap.vrchat.running, snap.vrchat.world.world_id.clone(), snap.vrchat.world.world_name.clone()),
            launcher: snap.launcher.arc(),
            account: snap.account.arc(),
            settings: snap.settings.arc(),
            capture: snap.capture.state,
            update: std::mem::discriminant(&snap.update.state),
            osc: (snap.osc.listening, snap.osc.advertising, snap.osc.port),
            lan: (std::mem::discriminant(&snap.lan.link), snap.lan.peer_label.clone()),
            call: snap.n4n0.calls.call.is_some(),
        }
    }
}

#[cfg(target_os = "macos")]
impl PartialEq for Told {
    fn eq(&self, other: &Told) -> bool {
        // (a part nothing changed is the very same one: looked into only where it is not)
        fn same<T: PartialEq>(a: &Arc<T>, b: &Arc<T>) -> bool {
            Arc::ptr_eq(a, b) || a == b
        }
        same(&self.launcher, &other.launcher)
            && same(&self.account, &other.account)
            && same(&self.settings, &other.settings)
            && (&self.vrchat, self.capture, self.update, self.osc, &self.lan, self.call) == (&other.vrchat, other.capture, other.update, other.osc, &other.lan, other.call)
    }
}

/// The Capture tab's name, as a window closed to the tray on it saved it and N4N0's assistant names it: it took its
/// place in Photos & captures (REQUEST21 5).
const CAPTURE_PAGE: &str = "Capture"; // i18n: not shown (the window's own page names)

/// Home's Groups section, as N4N0's assistant names it (REQUEST22 1): Home opens on it.
const GROUPS_SECTION: &str = "Groups"; // i18n: not shown (the window's own page names)

/// The page of a name [`App::follow_rest`] saved (its `Debug` name).
fn page_named(name: &str) -> Option<Page> {
    PAGES.into_iter().find(|p| format!("{p:?}") == name)
}

/// The window's pages, in the sidebar's order.
const PAGES: [Page; 8] = [Page::Home, Page::N4n0, Page::Projects, Page::Sessions, Page::Worlds, Page::Events, Page::Videos, Page::Settings];

/// Shows the window and gives it focus (a second launch or the tray asked for it).
fn bring_to_front(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

impl App {
    /// Offscreen frames call this instead of eframe's `logic`.
    pub fn logic_offline(&mut self, ctx: &egui::Context) {
        pages::watch::follow_window(ctx);
        self.drain_notices(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pages::capture::Part;

    /// Ninth verification: a window macOS alone put back from the Dock (its picture there pressed) is told to eframe, which holds
    /// it minimized for ever otherwise, and drawn at once where it is held hidden too; a change of what it shows as well.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_window_put_back_from_the_dock_by_macos_is_drawn_again() {
        use crate::mac_cover::{woken, Native, WATCHING};
        let _watching = WATCHING.lock().unwrap_or_else(|e| e.into_inner());
        let _ = woken();
        let mut app = App::offline(Arc::new(pages::capture::tests::Recorder::default()));
        let ctx = egui::Context::default();
        let pass = |app: &mut App, said: bool, occluded: bool, native: bool| {
            let info = egui::ViewportInfo { minimized: Some(said), occluded: Some(occluded), ..Default::default() };
            let input = egui::RawInput { viewports: std::iter::once((egui::ViewportId::ROOT, info)).collect(), ..Default::default() };
            let out = ctx.run_logic(&input, |ctx| {
                app.follow_cover(ctx, false, Native { id: Some(u32::MAX), minimized: Some(native) });
            });
            out.viewport_commands.into_values().flatten().collect::<Vec<_>>()
        };
        assert!(pass(&mut app, true, true, true).is_empty(), "in the Dock: nothing to tell");
        assert_eq!(pass(&mut app, true, false, false), [ViewportCommand::Minimized(false)]);
        std::thread::sleep(std::time::Duration::from_millis(300));
        app.cover = Default::default();
        assert_eq!(pass(&mut app, true, true, false), [ViewportCommand::Minimized(false), ViewportCommand::RequestPaintWhileHidden], "held hidden too");
        // Told, and hidden for certain (its number is none macOS lists): no more frames, until what it shows changes.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(pass(&mut app, false, true, false).is_empty());
        app.told.as_mut().unwrap().vrchat.0 ^= true;
        assert_eq!(pass(&mut app, false, true, false), [ViewportCommand::RequestPaintWhileHidden]);
        // Minimized by macOS alone (Command M): told as well.
        assert_eq!(pass(&mut app, false, false, true), [ViewportCommand::Minimized(true)]);
    }

    /// REQUEST21 5: the Capture tab left the sidebar; every way that opened it opens Photos & captures with its capture
    /// element open on the part it showed: a window closed to the tray on it, N4N0's assistant opening "the capture
    /// page", the sidebar's VRChats. Every other page's name still finds its page.
    #[test]
    fn every_link_of_the_capture_tab_opens_photos_and_captures_on_its_part() {
        let ctl: Ctl = Arc::new(pages::capture::tests::Recorder::default());
        assert!(page_named(CAPTURE_PAGE).is_none(), "no Capture page any more");
        assert!(!PAGES.iter().any(|p| format!("{p:?}") == CAPTURE_PAGE));
        for p in PAGES {
            assert_eq!(page_named(&format!("{p:?}")), Some(p));
        }
        // Closed to the tray on Capture, or the assistant's "open the capture page" (its window name is the tab's).
        assert_eq!(crate::controller::assistant::window_page("page:Capture"), Some(CAPTURE_PAGE));
        let mut app = App::offline(ctl.clone());
        assert!(app.go_named(CAPTURE_PAGE, 1.0));
        assert_eq!(app.page, Page::Sessions);
        assert!(app.capture.is_open());
        assert_eq!(app.capture.revealing(), Some(Part::Actions));
        // The sidebar's VRChats: on the one pictures are taken in.
        let mut app = App::offline(ctl.clone());
        app.open_capture(Part::Vrchats, 1.0);
        assert_eq!(app.page, Page::Sessions);
        assert_eq!(app.capture.revealing(), Some(Part::Vrchats));
        // Another page's name: that page, the element left as it was.
        let mut app = App::offline(ctl);
        assert!(app.go_named("Videos", 1.0));
        assert_eq!(app.page, Page::Videos);
        assert!(!app.capture.is_open());
        assert!(!app.go_named("Nowhere", 1.0));
    }

    /// verify-r22 Minor 4: N4N0's assistant's "open my groups" opens Home on its Groups section, and nothing more: the
    /// page's name carries no group and no write.
    #[test]
    fn the_assistant_opens_the_groups_section_and_nothing_more() {
        use orbiters_n4n0_core::intents::Page as AssistantPage;
        assert!(AssistantPage::AppGroups.on_window());
        assert_eq!(AssistantPage::of_key("app_groups"), Some(AssistantPage::AppGroups));
        let page = format!("page:{}", AssistantPage::AppGroups.window_name().unwrap());
        assert_eq!(crate::controller::assistant::window_page(&page), Some(GROUPS_SECTION));
        let ctl: Ctl = Arc::new(pages::capture::tests::Recorder::default());
        let mut app = App::offline(ctl);
        app.set_page(Page::Videos, 0.5);
        assert!(app.go_named(GROUPS_SECTION, 1.0));
        assert_eq!(app.page, Page::Home);
        let groups = app.home_mut().groups_mut();
        assert!(groups.reveal, "the section comes into view");
        assert!(groups.open.is_none() && groups.studio.is_none() && groups.flow.is_none(), "no group, no write");
    }
}

