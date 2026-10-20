//! The window: glow background, frameless title bar, sidebar with a sliding pill, the three pages with spring
//! entrances, toasts and the opening splash. The tray service does the work; this only reads its snapshot.

use crate::backend::Ctl;
use crate::controller::{Notice, NoticeKind};
use crate::pages::{self, Page};
use crate::state::{CaptureState, Snapshot};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, ResizeDirection, Sense, Ui, Vec2, ViewportCommand};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::glow::{self, Mood};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, logo, toast, widgets};
use std::sync::Arc;

const TITLE_H: f32 = 40.0;
const SIDEBAR_W: f32 = 232.0;
/// The splash shows for this long, then fades out over [`SPLASH_FADE`].
const SPLASH: f64 = 1.9;
const SPLASH_FADE: f64 = 0.55;

pub struct App {
    ctl: Ctl,
    /// False offscreen (screenshots): no window commands.
    windowed: bool,
    page: Page,
    page_since: f64,
    visit: u64,
    started: Option<f64>,
    show_splash: bool,
    sidebar_since: Option<f64>,
    toasts: toast::Toasts,
    capture: pages::capture::State,
    sessions: pages::sessions::State,
    settings: pages::settings::State,
    logo_glitch: Option<(f64, f32)>,
    logo_hovered: bool,
}

impl App {
    /// The window of the window process; `splash` plays the opening (first window since the app started).
    pub fn new(cc: &eframe::CreationContext<'_>, ctl: Ctl, splash: bool) -> Self {
        orbiters_ui::install(&cc.egui_ctx);
        if let Some(rs) = &cc.wgpu_render_state {
            orbiters_ui::install_wgpu(&rs.device, rs.target_format, 1, &mut rs.renderer.write().callback_resources);
        }
        ctl.attach(&cc.egui_ctx);
        #[cfg(windows)]
        round_corners(cc);
        let mut app = Self::offline(ctl);
        app.windowed = true;
        app.show_splash = splash;
        app
    }

    /// An app without a window (screenshots).
    pub fn offline(ctl: Ctl) -> Self {
        Self {
            ctl,
            windowed: false,
            page: Page::Capture,
            page_since: 0.0,
            visit: 0,
            started: None,
            show_splash: true,
            sidebar_since: None,
            toasts: toast::Toasts::default(),
            capture: Default::default(),
            sessions: Default::default(),
            settings: Default::default(),
            logo_glitch: None,
            logo_hovered: false,
        }
    }

    pub fn skip_splash(&mut self) {
        self.show_splash = false;
    }

    pub fn set_page(&mut self, page: Page, now: f64) {
        if page != self.page || self.visit == 0 {
            self.page = page;
            self.page_since = now;
            self.visit += 1;
        }
    }

    pub fn sessions_mut(&mut self) -> &mut pages::sessions::State {
        &mut self.sessions
    }

    pub fn toasts_mut(&mut self) -> &mut toast::Toasts {
        &mut self.toasts
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
                    if reason != "restarted" {
                        self.toasts.push(
                            ctx,
                            toast::Kind::Success,
                            format!(
                                "Session saved: {} frames, {keyframes} keyframes, {depth_frames} depth frames.",
                                orbiters_ui::format::integer(frames as f64)
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
        let now = anim::time(&ctx);
        let started = *self.started.get_or_insert(now);
        if self.visit == 0 {
            self.set_page(self.page, now);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        anim::set_live(&ctx, focused && !snap.settings.reduce_motion);

        let full = ui.max_rect();
        // A minimized window still runs a pass with a tiny rect: nothing to lay out.
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
            let page_rect = Rect::from_min_max(pos2(body.left() + SIDEBAR_W, body.top()), body.max);
            self.page(ui, &snap, page_rect);
        }
        self.title_bar(ui, &snap, title);
        self.toasts.show(&ctx);
        if splash_on {
            splash(&ctx, full, splash_t);
        }
        if self.windowed {
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
        let name = Txt::new("Orbiters desktop", 12.0).weight(weight::SEMIBOLD).tracking(0.025).color(theme::FG_2);
        let r = widgets::text(p, pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, &name);
        let subtitle = match snap.capture.state {
            CaptureState::Recording => Some("Recording".to_string()),
            CaptureState::Importing => Some("Importing".to_string()),
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

        // Window buttons, platform order.
        let close = if cfg!(windows) { "Close (Orbiters keeps running in the tray)" } else { "Quit" };
        let buttons = [(icons::MINUS, "Minimize", 15.0, false), (icons::SQUARE, "Maximize", 12.0, false), (icons::X, close, 16.0, true)];
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

        // Logo and name; hovering replays the signal glitch.
        let logo_rect = Rect::from_min_size(pos2(inner.left() + 8.0, inner.top() + 8.0), logo::size_for_width(36.0));
        let row = Rect::from_min_size(logo_rect.min, vec2(150.0, logo_rect.height()));
        let hovered = ui.interact(row, id.with("logo"), Sense::hover()).hovered();
        if hovered && !self.logo_hovered {
            self.logo_glitch = Some((now, 0.8));
        }
        self.logo_hovered = hovered;
        let intro = self.sidebar_intro(now);
        let recording = snap.capture.state == CaptureState::Recording;
        logo::paint(ui.painter(), logo_rect, logo::Look { intro_start: None, glitch: self.logo_glitch.or(intro), pulse: recording });
        let p = ui.painter();
        widgets::text(
            p,
            pos2(logo_rect.right() + 12.0, logo_rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            &Txt::new("Orbiters", 17.0).weight(weight::SEMIBOLD).tracking(-0.01),
        );
        widgets::text(p, pos2(logo_rect.right() + 12.0, logo_rect.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new("desktop", 11.5).color(theme::FG_3));

        // Navigation with a pill that slides to the active page.
        let nav_top = logo_rect.bottom() + 28.0;
        let items = [
            (Page::Capture, "Capture", icons::APERTURE),
            (Page::Sessions, "Sessions", icons::FOLDER_OPEN),
            (Page::Settings, "Settings", icons::SLIDERS_HORIZONTAL),
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
            p.set_opacity(alpha);
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

        // Status at the bottom.
        let bottom = inner.bottom() - 16.0;
        let version = Txt::new(format!("v{}", snap.version), 11.0).color(theme::FG_4);
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
            format!("OSC ready{}", osc.port.map(|p| format!(" · {p}")).unwrap_or_default())
        } else if osc.enabled {
            "OSC unavailable".into()
        } else {
            "OSC off".into()
        };
        let vrchat_text = if snap.vrchat.running { snap.vrchat.world.world_name.clone().unwrap_or_else(|| "Running".into()) } else { "Not running".into() };
        let vrchat_tone = if snap.vrchat.running { widgets::Tone::Ok } else { widgets::Tone::Off };
        side_status(ui, pos2(inner.left(), bottom - 34.0 - 44.0), "Action Menu", &osc_text, osc_tone, inner.width());
        side_status(ui, pos2(inner.left(), bottom - 34.0 - 88.0), "VRChat", &vrchat_text, vrchat_tone, inner.width());
    }

    /// The intro glitch of the sidebar logo, once, right after the splash.
    fn sidebar_intro(&self, now: f64) -> Option<(f64, f32)> {
        let since = self.sidebar_since?;
        (now - since < 1.2).then_some((since + 0.15, 1.0))
    }

    fn page(&mut self, ui: &mut Ui, snap: &Arc<Snapshot>, rect: Rect) {
        let ctx = ui.ctx().clone();
        // Enter: slide in from the right and fade up with a soft spring.
        let v = anim::spring_from(&ctx, Id::unique(("orbiters-page", self.visit)), [0.0], [1.0], Spring::SOFT)[0];
        let shifted = rect.translate(vec2(24.0 * (1.0 - v), 0.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(shifted).id_salt(("page", self.visit)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child.multiply_opacity(v.clamp(0.0, 1.0));
        let mut cx = pages::Cx { ctl: &self.ctl, snap, toasts: &mut self.toasts, entered: self.page_since, visit: self.visit, windowed: self.windowed };
        match self.page {
            Page::Capture => pages::capture::show(&mut child, &mut cx, &mut self.capture),
            Page::Sessions => pages::sessions::show(&mut child, &mut cx, &mut self.sessions),
            Page::Settings => pages::settings::show(&mut child, &mut cx, &mut self.settings),
        }
    }
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
        p.set_opacity(fade);
        p.rect_filled(full, 0.0, theme::INK_950);
        let start = anim::time(ctx) - t;
        let scale_k = anim::spring_from(ctx, Id::unique("orbiters-splash-scale"), [0.82], [1.0], Spring { stiffness: 180.0, damping: 18.0, mass: 1.0 })[0];
        let lift = 1.0 + 0.04 * (1.0 - fade);
        let size = logo::size_for_width(128.0) * scale_k * lift;
        let center = full.center() - vec2(0.0, 36.0);
        let logo_rect = Rect::from_center_size(center, size);
        let mut lp = p.clone();
        lp.multiply_opacity(((t / 0.35) as f32).clamp(0.0, 1.0));
        logo::paint(&lp, logo_rect, logo::Look { intro_start: Some(start), glitch: Some((start + 0.3, 1.2)), pulse: false });
        let k = ease::segment(t as f32, 0.75, 0.6, ease::out_expo);
        let mut tp = p.clone();
        tp.multiply_opacity(k);
        let y = logo_rect.bottom() + 24.0 + 14.0 * (1.0 - k);
        widgets::text(&tp, pos2(full.center().x, y + 16.0), Align2::CENTER_CENTER, &Txt::new("Orbiters", 28.0).weight(weight::SEMIBOLD).tracking(-0.01));
        let galley = tp.layout_job(Txt::new("DESKTOP", 13.0).weight(weight::MEDIUM).tracking(0.32).color(theme::FG_3).job(f32::INFINITY));
        let at = pos2(full.center().x - galley.size().x * 0.5 + 0.16 * 13.0, y + 44.0 - galley.size().y * 0.5);
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
        if self.ctl.should_close() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.drain_notices(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
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
        self.drain_notices(ctx);
    }
}
