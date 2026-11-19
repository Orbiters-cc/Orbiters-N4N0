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
    home: pages::home::State,
    capture: pages::capture::State,
    projects: pages::projects::State,
    sessions: pages::sessions::State,
    worlds: pages::worlds::State,
    events: pages::events::State,
    videos: pages::videos::State,
    settings: pages::settings::State,
    n4n0: pages::n4n0::State,
    logo_glitch: Option<(f64, f32)>,
    logo_hovered: bool,
    /// The page (and Home's feature) last told to the usage statistics.
    usage_here: Option<(Page, Option<orbiters_n4n0_core::menu::Page>)>,
    /// The usage statistics' notice was closed here (the service's settings say so a moment later).
    usage_told: bool,
}

impl App {
    /// The window of the window process; `splash` plays the opening (first window since the app started).
    pub fn new(cc: &eframe::CreationContext<'_>, ctl: Ctl, splash: bool) -> Self {
        orbiters_ui::install(&cc.egui_ctx);
        if let Some(rs) = &cc.wgpu_render_state {
            crate::gpu::report_errors(&rs.device);
            orbiters_ui::install_wgpu(&rs.device, rs.target_format, 1, &mut rs.renderer.write().callback_resources);
        }
        ctl.attach(&cc.egui_ctx);
        #[cfg(windows)]
        round_corners(cc);
        #[cfg(target_os = "macos")]
        crate::mac::install_window_menu(ctl.clone());
        let mut app = Self::offline(ctl);
        app.windowed = true;
        app.gpu = cc.wgpu_render_state.as_ref().map(crate::gpu::Handles::of_window);
        app.show_splash = splash;
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
            home: Default::default(),
            capture: Default::default(),
            projects: Default::default(),
            sessions: Default::default(),
            worlds: Default::default(),
            events: Default::default(),
            videos: Default::default(),
            settings: Default::default(),
            n4n0: Default::default(),
            logo_glitch: None,
            logo_hovered: false,
            usage_here: None,
            usage_told: false,
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
        orbiters_ui::fonts::follow_language(&ctx);
        // This process reads Orbiters too (the Map explorer's public pages): from the environment the service uses,
        // production included (else Unity's switch would decide).
        crate::controller::follow_environment(snap.settings.development);
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
        glow::paint(ui.painter(), full, mood);

        let splash_t = now - started;
        let splash_on = self.show_splash && splash_t < SPLASH + SPLASH_FADE;
        let content_ready = !self.show_splash || splash_t >= SPLASH;

        let title = Rect::from_min_size(full.min, vec2(full.width(), TITLE_H));
        let body = Rect::from_min_max(pos2(full.left(), full.top() + TITLE_H), full.max);
        if content_ready {
            let since = *self.sidebar_since.get_or_insert(now);
            self.sidebar(ui, &snap, Rect::from_min_size(body.min, vec2(SIDEBAR_W, body.height())), since);
            // From the top of the window: the content scrolls on under the title bar (see `pages::under_top`).
            let page_rect = Rect::from_min_max(pos2(body.left() + SIDEBAR_W, full.top()), body.max);
            self.page(ui, &snap, page_rect);
            let told = self.usage_told || snap.settings.usage.told && snap.settings.usage.told_perf;
            if let Some(answer) = pages::usage_notice::show(ui, page_rect, told) {
                self.usage_told = true;
                self.ctl.patch_settings(serde_json::json!({ "usage": { "told": true, "told_perf": true } }));
                if answer == pages::usage_notice::Answer::Settings {
                    self.set_page(Page::Settings, now);
                }
            }
        }
        self.title_bar(ui, &snap, title);
        self.toasts.show(&ctx);
        // "Talk with N4N0?": the wake word heard during a call.
        crate::wake_offer::window(&ctx, &self.ctl, snap.n4n0.voice.wake_offer);
        // "No 3D picture of this world yet" (REQUEST13), over that card when both are up.
        let talk_up = crate::wake_offer::up(snap.n4n0.voice.wake_offer, crate::wake_offer::now_ms()).is_some();
        crate::world_notice::window(&ctx, &self.ctl, snap.explorer.notice.as_ref(), talk_up);
        if splash_on {
            splash(&ctx, full, splash_t);
        }
        // The Mac's window has edges of its own (window.rs).
        if self.windowed && cfg!(not(target_os = "macos")) {
            resize_handles(ui, full);
        }
    }

    fn title_bar(&mut self, ui: &mut Ui, snap: &Snapshot, rect: Rect) {
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
        let name = Txt::new("Orbiters - N4N0", 12.0).weight(weight::SEMIBOLD).tracking(0.025).color(theme::FG_2);
        let r = widgets::text(p, pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, &name);
        // The beta's chip after the app's name, over every page (Enzo, 2026-11-15).
        let r = r.union(beta_chip(ui, pos2(r.right() + 8.0, rect.center().y)));
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
            &Txt::new("", 12.0).color(theme::FG_3),
            420.0,
        );

        // While a recording runs, its Stop from every page (Enzo, F13): never a start.
        let live = !snap.capture.photo_3d && matches!(snap.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Stopping);
        if live {
            let stopping = snap.capture.state == CaptureState::Stopping;
            let b = widgets::Button::new(if stopping { tr("Stopping…") } else { tr("Stop recording") }).icon(icons::RECORD).variant(widgets::Variant::Stop).size(widgets::Size::Sm).enabled(!stopping);
            let w = b.desired_width(ui);
            let at = Rect::from_min_size(pos2(rect.right() - 4.0 - 44.0 * 3.0 - 12.0 - w, rect.top() + 6.0), vec2(w, 28.0));
            if widgets::activated(&b.show_at(ui, at)) {
                self.ctl.stop_capture();
            }
        }
        // Window buttons, platform order.
        let close = if cfg!(windows) {
            tr("Close (Orbiters keeps running in the tray)")
        } else if cfg!(target_os = "macos") {
            tr("Close (Orbiters keeps running in the menu bar)")
        } else {
            tr("Quit")
        };
        let buttons = [(icons::MINUS, tr("Minimize"), 15.0, false), (icons::SQUARE, tr("Maximize"), 12.0, false), (icons::X, close, 16.0, true)];
        for (i, (glyph, label, size, danger)) in buttons.into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(rect.right() - 4.0 - 44.0 * (3 - i) as f32, rect.top() + 4.0), vec2(44.0, 32.0));
            let resp = ui.interact(b, id.with(i), Sense::click()).on_hover_text(label);
            let hover = anim::toggle(&ctx, id.with(("hover", i)), resp.hovered(), Spring::SNAPPY);
            let press = anim::toggle(&ctx, id.with(("press", i)), resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let br = Rect::from_center_size(b.center(), b.size() * (1.0 - 0.14 * press));
            let (bg, fg) = if danger {
                (theme::alpha(theme::REC, 0.85 * hover), theme::mix(theme::FG_3, egui::Color32::WHITE, hover))
            } else {
                (theme::white(0.08 * hover), theme::mix(theme::FG_3, theme::FG, hover))
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
        widgets::text(p, pos2(logo_rect.right() + 12.0, logo_rect.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new("Orbiters", 11.5).color(theme::FG_3));
        let name_wide = widgets::text_size(ui, &Txt::new("N4N0", 17.0).weight(weight::SEMIBOLD).tracking(0.02)).x.max(widgets::text_size(ui, &Txt::new("Orbiters", 11.5)).x);
        self.update_button(ui, snap, pos2(inner.right(), logo_rect.center().y), logo_rect.right() + 12.0 + name_wide + 8.0, id.with("update"));

        // Navigation with a pill that slides to the active page.
        let nav_top = logo_rect.bottom() + 28.0;
        let items = [
            (Page::Home, tr("Home"), icons::HOUSE),
            (Page::N4n0, "N4N0", icons::N4N0),
            (Page::Capture, tr("Capture"), icons::APERTURE),
            (Page::Projects, tr("Unity projects"), icons::CUBE),
            (Page::Sessions, tr("Photos & captures"), icons::IMAGES),
            (Page::Worlds, tr("Visited worlds"), icons::GLOBE),
            (Page::Events, tr("Events"), icons::CALENDAR_DOTS),
            (Page::Videos, tr("Videos"), icons::FILM_STRIP),
            (Page::Settings, tr("Settings"), icons::SLIDERS_HORIZONTAL),
        ];
        let item_rect = |i: usize| Rect::from_min_size(pos2(inner.left(), nav_top + i as f32 * 48.0), vec2(inner.width(), 44.0));
        let active = items.iter().position(|(pg, _, _)| *pg == self.page).unwrap_or(0);
        let pill_y = anim::value(&ctx, id.with("pill"), (item_rect(active).top() - nav_top) as f32, Spring::BOUNCY);
        let pill = Rect::from_min_size(pos2(inner.left(), nav_top + pill_y), vec2(inner.width(), 44.0));
        let p = ui.painter();
        p.rect_filled(pill, theme::radius(14.0), theme::white(0.07));
        let bar = Rect::from_center_size(pos2(pill.left() + 1.5, pill.center().y), vec2(3.0, 20.0));
        p.rect_filled(bar, theme::radius(1.5), theme::ACCENT);

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
                theme::FG_2
            } else {
                theme::FG_3
            };
            let fg = anim::color(&ctx, id.with(("fg", i)), target, Spring::SNAPPY);
            let icon_c = anim::color(&ctx, id.with(("ic", i)), if is_active { theme::ACCENT } else { target }, Spring::SNAPPY);
            let mut p = ui.painter().clone();
            p.multiply_opacity(alpha);
            let x = r.left() + 14.0 + 2.0 * press;
            if is_active {
                widgets::icon_fill(&p, pos2(x + 9.0, r.center().y), glyph, 18.0, icon_c);
            } else {
                widgets::icon(&p, pos2(x + 9.0, r.center().y), glyph, 18.0, icon_c);
            }
            widgets::text(&p, pos2(x + 30.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(label, 14.0).weight(weight::MEDIUM).color(fg));
            if page == Page::Capture && capturing {
                let tone = if snap.capture.state == CaptureState::Importing { widgets::Tone::Info } else { widgets::Tone::Rec };
                widgets::status_dot(&p, pos2(r.right() - 16.0, r.center().y), tone, true);
            }
            if page == Page::Sessions && snap.storage.sessions > 0 {
                let t = Txt::new(snap.storage.sessions.to_string(), 11.0).color(theme::FG_3);
                let s = widgets::text_size(ui, &t);
                let badge = Rect::from_min_size(pos2(r.right() - 12.0 - s.x - 16.0, r.center().y - 10.0), vec2(s.x + 16.0, 20.0));
                p.rect_filled(badge, theme::radius(10.0), theme::white(0.07));
                widgets::text(&p, badge.center(), Align2::CENTER_CENTER, &t);
            }
        }

        // Status at the bottom: the version, and the development environment when it is the one in use.
        let bottom = inner.bottom() - 16.0;
        let version = if snap.settings.development { tr!("v{version} · development", version = snap.version) } else { format!("v{}", snap.version) };
        let version = Txt::new(version, 11.0).color(if snap.settings.development { theme::WARN } else { theme::FG_4 });
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
        // opens the Capture page, where another is chosen.
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
                self.set_page(Page::Capture, now);
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
        let words = |size: f32| Txt::new(label, size).weight(weight::SEMIBOLD).color(if live { theme::ON_ACCENT } else { theme::ACCENT });
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
        let fill = if live { theme::mix(theme::ACCENT, theme::ACCENT_HOVER, hover) } else { theme::alpha(theme::ACCENT, 0.14) };
        p.rect_filled(shown, theme::radius(13.0), fill);
        let at = pos2(shown.left() + 13.0, shown.center().y);
        match fraction {
            Some(f) => {
                orbiters_ui::paint::ring(p, at, 5.0, 2.0, theme::alpha(theme::ACCENT, 0.3));
                orbiters_ui::paint::arc(p, at, 5.0, -std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * f.clamp(0.0, 1.0), 2.0, theme::ACCENT);
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            None if with_icon => widgets::icon(p, at, icons::DOWNLOAD_SIMPLE, 12.5, if live { theme::ON_ACCENT } else { theme::ACCENT }),
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
        match self.page {
            Page::Home => pages::home::show(&mut child, &mut cx, &mut self.home),
            Page::Capture => pages::capture::show(&mut child, &mut cx, &mut self.capture, &mut self.projects),
            Page::Projects => pages::projects::show(&mut child, &mut cx, &mut self.projects),
            Page::N4n0 => pages::n4n0::show(&mut child, &mut cx, &mut self.n4n0),
            Page::Sessions => pages::sessions::show(&mut child, &mut cx, &mut self.sessions),
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
                pages::Go::VrchatAccount => self.set_page(Page::Settings, anim::time(&ctx)),
            }
        }
    }
}

/// The chip that says this is a beta, its left end at `left_center`: the accent on a tint of it, as the website's badges are.
pub(crate) fn beta_chip(ui: &Ui, left_center: egui::Pos2) -> Rect {
    let word = orbiters_i18n::tr_ctx("chip", "Beta").to_uppercase();
    let txt = Txt::new(word, 10.0).weight(weight::BOLD).tracking(0.06).color(theme::ACCENT);
    let size = widgets::text_size(ui, &txt);
    let chip = Rect::from_min_size(pos2(left_center.x, left_center.y - 9.0), vec2(size.x + 14.0, 18.0));
    let p = ui.painter();
    p.rect_filled(chip, theme::radius(9.0), theme::alpha(theme::ACCENT, 0.16));
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
    widgets::text_truncated(p, pos2(row.left() + 30.0, row.center().y + 8.0), Align2::LEFT_CENTER, &Txt::new(text, 12.0).color(theme::FG_2), width - 40.0);
}

/// Opening moment: the mark draws itself in with its signal echoes, the name rises, then everything lifts away.
fn splash(ctx: &egui::Context, full: Rect, t: f64) {
    let fade = if t > SPLASH { 1.0 - ((t - SPLASH) / SPLASH_FADE) as f32 } else { 1.0 };
    let fade = ease::out_cubic(fade.clamp(0.0, 1.0));
    if t < SPLASH + SPLASH_FADE {
        ctx.request_repaint();
    }
    egui::Area::new(Id::unique("orbiters-splash")).order(egui::Order::Foreground).fixed_pos(full.min).interactable(true).show(ctx, |ui| {
        let (_, _) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
        let mut p = ui.painter().clone();
        p.multiply_opacity(fade);
        p.rect_filled(full, 0.0, theme::INK_950);
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
        let galley = tp.layout_job(Txt::new("ORBITERS", 13.0).weight(weight::MEDIUM).tracking(0.32).color(theme::FG_3).job(f32::INFINITY));
        let at = pos2(full.center().x - galley.size().x * 0.5 + 0.16 * 13.0, y + 6.0 - galley.size().y * 0.5);
        tp.galley(at, galley, theme::FG);
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
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.ctl.take_show() {
            bring_to_front(ctx);
        }
        // Asked from the menus around VRChat: My Avatar, to add the capture kit, is on Projects.
        if let Some(page) = self.ctl.take_show_page() {
            if page == "projects" {
                self.set_page(Page::Projects, anim::time(ctx));
            } else if let Some(dir) = page.strip_prefix("share-to:") {
                // "Share to …" beside VRChat's camera on the desktop: this window's own share dialog, on its friends.
                let server = if self.ctl.view().settings.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                if let Some(entry) = crate::controller::shares::session_entry(std::path::Path::new(dir), server) {
                    self.set_page(Page::Sessions, anim::time(ctx));
                    self.sessions.gallery_mut().open_share_to(ctx, entry);
                    bring_to_front(ctx);
                }
            } else if let Some(dir) = page.strip_prefix("session:") {
                // Asked from a capture turning on a sheet: it opens here, in the app's own 3D viewer.
                self.set_page(Page::Sessions, anim::time(ctx));
                let middle = ctx.content_rect().center();
                self.sessions.open_session(ctx, std::path::PathBuf::from(dir), Rect::from_center_size(middle, vec2(300.0, 200.0)));
            }
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        self.ctl.set_background(!focused && self.ctl.view().vrchat.runs_here());
        // Minimized, no frame is drawn and only this runs: what the pages watch on their threads stops until the window
        // shows again (the microphones Settings listens to, Unity's editors, the folders).
        pages::watch::follow_window(ctx);
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
        crate::usage::flush(&self.ctl);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::INK_950.to_normalized_gamma_f32()
    }
}

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
