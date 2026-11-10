//! `orbiters-desktop --gpu-bench <scenario> [--gpu integrated|graphics-card] [--seconds <s>] [--warmup <s>]
//! [--session <name>] [--publish <hz> [--background]] [--size <w>x<h>] [--position <x>,<y>] [--alternate <s>]
//! [--watches]` opens
//! the real window on the GPU asked for, over a stand-in controller (the screenshots' made-up snapshot and the real
//! session library; the controller is not started), without taking the focus, drives it with synthetic input and
//! prints its frames each second and a summary: what docs/igpu-experiment.md measured with scripts/gpu-lab.
//!
//! Scenarios: `idle` (Home, nothing moves: its music is looked at on a thread, which asks for a frame when it
//! changed; `--idle-frames` counts every page's frames at rest without a window), `still` (Settings, no frame at all), `live` (Home as if focused, so looping animations run), `scroll` (Settings scrolled up and down), `pages`
//! (another page every second), `splats` (the 3D view of the session whose folder name contains `--session`,
//! orbiting), `splats-still` (that 3D view, still). `--publish` republishes the snapshot that many times a second, as
//! the tray service does while recording or with N4N0's menu open; with `--background` it is drawn as a window behind
//! VRChat draws it ([`crate::ipc::BACKGROUND_PACE`]). `--alternate` has `scroll` and `splats` move for that long, then
//! rest as long, and so on. `--view-gpu always` draws the 3D view's splats on the integrated GPU while the window draws
//! on the graphics card (`settings::ViewGpu`; `never`, the default here, keeps them on the window's GPU). `--watches`
//! adds, each second, how many of the pages' watches look (`pages::watch`): with `still` (Settings, whose watch keeps
//! the microphones open) and the window minimized then restored by hand, it shows that they end and start again.

use crate::app::App;
use crate::backend::{Ctl, Local};
use crate::controller::{self, Controller};
use crate::pages::Page;
use anyhow::Context as _;
use egui::{pos2, vec2, Event, Pos2, RawInput, Rect, ViewportCommand, ViewportId};
use orbiters_core::settings::{Settings, ViewGpu, WindowGpu};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `--watches`: the count of the pages' watches is printed each second.
static WATCHES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scenario {
    Idle,
    Still,
    Live,
    Scroll,
    Pages,
    Splats,
    SplatsStill,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let scenario = match args.first().map(String::as_str) {
        Some("idle") => Scenario::Idle,
        Some("still") => Scenario::Still,
        Some("live") => Scenario::Live,
        Some("scroll") => Scenario::Scroll,
        Some("pages") => Scenario::Pages,
        Some("splats") => Scenario::Splats,
        Some("splats-still") => Scenario::SplatsStill,
        _ => anyhow::bail!("usage: --gpu-bench idle|still|live|scroll|pages|splats|splats-still [--gpu integrated|graphics-card] [--seconds s] [--warmup s] [--session name] [--publish hz [--background]] [--size WxH] [--position x,y] [--alternate s] [--view-gpu always|never]"),
    };
    let mut gpu = WindowGpu::IntegratedGpu;
    let mut view_gpu = ViewGpu::Never;
    let (mut seconds, mut warmup, mut publish, mut alternate) = (30.0f64, 3.0f64, 0.0f64, 0.0f64);
    let mut size = [1280.0f32, 820.0];
    let mut background = false;
    let mut position: Option<[f32; 2]> = None;
    let mut session = "New Generation".to_string();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gpu" => {
                gpu = match it.next().map(String::as_str) {
                    Some("integrated") => WindowGpu::IntegratedGpu,
                    Some("graphics-card") => WindowGpu::GraphicsCard,
                    _ => anyhow::bail!("--gpu integrated|graphics-card"),
                }
            }
            "--view-gpu" => {
                view_gpu = match it.next().map(String::as_str) {
                    Some("always") => ViewGpu::Always,
                    Some("never") => ViewGpu::Never,
                    _ => anyhow::bail!("--view-gpu always|never"),
                }
            }
            "--seconds" => seconds = it.next().context("--seconds <s>")?.parse()?,
            "--warmup" => warmup = it.next().context("--warmup <s>")?.parse()?,
            "--publish" => publish = it.next().context("--publish <hz>")?.parse()?,
            "--background" => background = true,
            "--watches" => WATCHES.store(true, std::sync::atomic::Ordering::Relaxed),
            "--alternate" => alternate = it.next().context("--alternate <s>")?.parse()?,
            "--session" => session = it.next().context("--session <name>")?.clone(),
            "--size" => {
                let (w, h) = it.next().and_then(|s| s.split_once('x')).context("--size <w>x<h>")?;
                size = [w.parse()?, h.parse()?];
            }
            "--position" => {
                let (x, y) = it.next().and_then(|s| s.split_once(',')).context("--position <x>,<y>")?;
                position = Some([x.parse()?, y.parse()?]);
            }
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    let settings = Settings { consent_acknowledged: true, view_gpu, ..Default::default() };
    let sessions = controller::list_sessions(&settings.sessions_dir);
    let chosen = sessions.iter().find(|s| s.dir.file_name().is_some_and(|n| n.to_string_lossy().contains(&session))).map(|s| s.dir.clone());
    if matches!(scenario, Scenario::Splats | Scenario::SplatsStill) && chosen.is_none() {
        anyhow::bail!("no session named like {session} in {}", settings.sessions_dir.display());
    }
    let ctl = Controller::new(settings.clone());
    let snapshot = crate::screenshot::idle_snapshot(&settings, &sessions);
    ctl.set_snapshot(snapshot.clone());
    let news = (publish > 0.0).then(|| News { ctl: ctl.clone(), snapshot, hz: publish, background });
    let backend: Ctl = Arc::new(Local::new(ctl));
    println!("pid {}", std::process::id());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Orbiters - N4N0 GPU bench")
        .with_app_id("orbiters-desktop-bench")
        .with_decorations(false)
        .with_inner_size(size)
        .with_active(false)
        .with_icon(crate::app::window_icon());
    if let Some(p) = position {
        viewport = viewport.with_position(p);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: position.is_none(),
        wgpu_options: crate::gpu::options(gpu),
        system_font_fallback: false,
        ..Default::default()
    };
    eframe::run_native(
        "Orbiters - N4N0 GPU bench",
        options,
        Box::new(move |cc| {
            let bench = Bench::new(cc, backend, scenario, chosen, seconds, warmup, alternate, view_gpu == ViewGpu::Always && gpu == WindowGpu::GraphicsCard);
            if let Some(news) = news {
                news.start(&cc.egui_ctx);
            }
            Ok(Box::new(bench))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The tray service's news, republished `hz` times a second.
struct News {
    ctl: controller::Shared,
    snapshot: crate::state::Snapshot,
    hz: f64,
    /// Drawn as by a window behind VRChat ([`crate::ipc::BACKGROUND_PACE`]) instead of a frame each.
    background: bool,
}

impl News {
    fn start(self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        if self.background {
            // The controller asks for a frame on every change: it asks a context nobody draws instead.
            self.ctl.attach(&egui::Context::default());
        }
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs_f64(1.0 / self.hz));
            self.ctl.set_snapshot(self.snapshot.clone());
            if self.background {
                ctx.request_repaint_after(crate::ipc::BACKGROUND_PACE);
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    /// The page shows; the 3D view waits for the session list.
    Start,
    /// The session's detail opened at this time.
    Detail(f64),
    /// The 3D view loads its splats since this time.
    Loading(f64),
    /// Warming up until this time, then measuring.
    Warmup(f64),
    /// Measuring since this time.
    Measure(f64),
}

struct Bench {
    app: App,
    gpu: Option<crate::gpu::Handles>,
    scenario: Scenario,
    session: Option<PathBuf>,
    seconds: f64,
    warmup: f64,
    stage: Stage,
    /// Frames this second, the intervals between frames while measuring, the last frame.
    frames: Arc<Mutex<Frames>>,
    next_page: f64,
    pages: usize,
    pressed: bool,
    /// Seconds of moving, then of rest, in turn (0: always moving).
    alternate: f64,
    /// The 3D view's splats on the integrated GPU.
    view_integrated: bool,
}

#[derive(Default)]
struct Frames {
    this_second: u32,
    intervals: Vec<f64>,
    last: Option<Instant>,
}

impl Bench {
    #[allow(clippy::too_many_arguments)]
    fn new(cc: &eframe::CreationContext<'_>, backend: Ctl, scenario: Scenario, session: Option<PathBuf>, seconds: f64, warmup: f64, alternate: f64, view_integrated: bool) -> Self {
        let mut app = App::new(cc, backend, false);
        app.skip_splash();
        if let Some(rs) = &cc.wgpu_render_state {
            let info = rs.adapter.get_info();
            println!("adapter {} ({:?}, {:04x}:{:04x}), {:?}", info.name, info.device_type, info.vendor, info.device, rs.target_format);
        }
        let gpu = cc.wgpu_render_state.as_ref().map(crate::gpu::Handles::of_window);
        let page = match scenario {
            Scenario::Idle | Scenario::Live | Scenario::Pages => Page::Home,
            Scenario::Scroll | Scenario::Still => Page::Settings,
            Scenario::Splats | Scenario::SplatsStill => Page::Sessions,
        };
        app.set_page(page, 0.0);
        Self {
            app,
            gpu,
            scenario,
            session,
            seconds,
            warmup,
            stage: Stage::Start,
            frames: Default::default(),
            next_page: 0.0,
            pages: 0,
            pressed: false,
            alternate,
            view_integrated,
        }
    }

    fn measuring(&self) -> Option<f64> {
        match self.stage {
            Stage::Measure(t) => Some(t),
            _ => None,
        }
    }

    /// The viewer's centre (the 3D view covers the window below the title bar).
    fn centre(ctx: &egui::Context) -> Pos2 {
        let r = ctx.content_rect();
        pos2(r.center().x, (r.top() + crate::app::TITLE_H + r.bottom()) * 0.5)
    }

    /// Prints the frames each second from another thread (so that nothing wakes the window to count), then the
    /// summary, and closes the window.
    fn watch(&self, ctx: &egui::Context) {
        let (frames, seconds, ctx) = (self.frames.clone(), self.seconds, ctx.clone());
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut tick = start;
            let views = || crate::pages::splat_worker::DRAWN.load(std::sync::atomic::Ordering::Relaxed);
            let (views_start, mut views_last) = (views(), views());
            while start.elapsed().as_secs_f64() < seconds {
                tick += Duration::from_secs(1);
                std::thread::sleep(tick.saturating_duration_since(Instant::now()));
                let n = std::mem::take(&mut frames.lock().unwrap().this_second);
                let k = views() - views_last;
                views_last += k;
                if k > 0 {
                    println!("{n} frames, {k} of the 3D view from the integrated GPU");
                } else {
                    println!("{n} frames");
                }
                if WATCHES.load(std::sync::atomic::Ordering::Relaxed) {
                    println!("{} watches at {:.0} s", crate::pages::watch::live(), start.elapsed().as_secs_f64());
                }
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            let (mut v, drawn) = {
                let f = frames.lock().unwrap();
                (f.intervals.clone(), f.intervals.len() + f.last.is_some() as usize)
            };
            v.sort_by(f64::total_cmp);
            let n = v.len();
            let pick = |p: f64| if n == 0 { f64::NAN } else { v[((n - 1) as f64 * p).round() as usize] };
            println!(
                "summary: {drawn} frames in {seconds:.1} s ({:.1} a second); frame interval ms p50 {:.2} p95 {:.2} p99 {:.2} max {:.2}",
                drawn as f64 / seconds,
                pick(0.5),
                pick(0.95),
                pick(0.99),
                v.last().copied().unwrap_or(f64::NAN)
            );
            let k = views() - views_start;
            if k > 0 {
                println!("3D view: {k} frames from the integrated GPU ({:.1} a second)", k as f64 / seconds);
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
            ctx.send_viewport_cmd(ViewportCommand::Close);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for Bench {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        let Some(since) = self.measuring() else { return };
        let t = raw.time.unwrap_or(0.0) - since;
        let moving = self.alternate <= 0.0 || (t / self.alternate) as i64 % 2 == 0;
        if !moving {
            if self.pressed {
                self.pressed = false;
                let p = Self::centre(ctx);
                raw.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
            }
            return;
        }
        match self.scenario {
            Scenario::Live => {
                raw.focused = true;
                raw.viewports.entry(ViewportId::ROOT).or_default().focused = Some(true);
                // Someone at the window: looping decorations rest after half a minute without input.
                if self.pages != (t / 5.0) as usize {
                    self.pages = (t / 5.0) as usize;
                    raw.events.push(Event::PointerMoved(pos2(900.0 + self.pages as f32, 700.0)));
                }
            }
            Scenario::Scroll => {
                let delta = if (t / 1.5) as i64 % 2 == 0 { -40.0 } else { 40.0 };
                raw.events.push(Event::PointerMoved(pos2(760.0, 450.0)));
                raw.events.push(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                    source: egui::MouseWheelSource::Wheel,
                });
                ctx.request_repaint();
            }
            Scenario::Splats => {
                let c = Self::centre(ctx);
                let a = t as f32 * 1.2;
                let p = c + vec2(160.0 * a.cos(), 50.0 * (2.0 * a).sin());
                if !self.pressed {
                    self.pressed = true;
                    raw.events.push(Event::PointerMoved(c + vec2(160.0, 0.0)));
                    raw.events.push(Event::PointerButton {
                        pos: c + vec2(160.0, 0.0),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                raw.events.push(Event::PointerMoved(p));
                ctx.request_repaint();
            }
            Scenario::Idle | Scenario::Still | Scenario::Pages | Scenario::SplatsStill => {}
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        eframe::App::logic(&mut self.app, ctx, frame);
        let now = ctx.input(|i| i.time);
        match self.stage {
            Stage::Start => match self.scenario {
                Scenario::Splats | Scenario::SplatsStill => {
                    if self.app.sessions_mut().loaded() {
                        if let Some(dir) = self.session.clone() {
                            self.app.sessions_mut().open_session(ctx, dir, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
                        }
                        self.stage = Stage::Detail(now);
                    }
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
                _ => {
                    self.stage = Stage::Warmup(now + self.warmup);
                    ctx.request_repaint();
                }
            },
            Stage::Detail(at) => {
                if now - at > 1.0 {
                    if let Some(gpu) = self.gpu.clone() {
                        self.app.sessions_mut().open_viewer(ctx, &gpu, self.view_integrated);
                    }
                    self.stage = Stage::Loading(now);
                }
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Stage::Loading(at) => {
                if self.app.sessions_mut().viewer_loaded() {
                    println!("3D view loaded in {:.1} s", now - at);
                    self.stage = Stage::Warmup(now + self.warmup);
                }
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Stage::Warmup(until) => {
                if now >= until {
                    let size = ctx.content_rect().size() * ctx.pixels_per_point();
                    println!("measuring {:.0} s, {:.0}x{:.0} pixels", self.seconds, size.x, size.y);
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    self.stage = Stage::Measure(now);
                    *self.frames.lock().unwrap() = Frames::default();
                    self.next_page = now;
                    self.watch(ctx);
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(Duration::from_secs_f64(until - now));
                }
            }
            Stage::Measure(since) => {
                // Wakes up when the next turn of moving starts.
                if self.alternate > 0.0 {
                    let next = since + ((now - since) / self.alternate).floor() * self.alternate + self.alternate;
                    ctx.request_repaint_after(Duration::from_secs_f64((next - now).max(0.001)));
                }
                if self.scenario == Scenario::Pages {
                    if now >= self.next_page {
                        const PAGES: [Page; 6] = [Page::N4n0, Page::Capture, Page::Sessions, Page::Worlds, Page::Settings, Page::Home];
                        self.app.set_page(PAGES[self.pages % PAGES.len()], orbiters_ui::anim::time(ctx));
                        self.pages += 1;
                        self.next_page += 1.0;
                    }
                    ctx.request_repaint_after(Duration::from_secs_f64((self.next_page - now).max(0.0)));
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
        if self.measuring().is_some() {
            let now = Instant::now();
            let mut f = self.frames.lock().unwrap();
            if let Some(last) = f.last {
                f.intervals.push((now - last).as_secs_f64() * 1e3);
            }
            f.last = Some(now);
            f.this_second += 1;
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        eframe::App::clear_color(&self.app, visuals)
    }
}
