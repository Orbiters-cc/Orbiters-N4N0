//! The setup's window, in the app's look (its theme, fonts, glow background, logo and widgets): the brand on the
//! left, the step on the right, entering as the app's pages do. It runs a task of [`crate::install`] on a thread and
//! shows its progress; the window cannot be closed while one runs.

use crate::install::{self, Options, Progress};
use crate::{Args, Mode};
use egui::{pos2, vec2, Align, Align2, Color32, Id, Layout, Rect, Sense, Shape, Ui, UiBuilder, ViewportCommand};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::glow::{self, Mood};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use orbiters_ui::{format, icons, logo, paint};
use orbiters_update::{payload::Index, InstallRecord, DISPLAY_NAME, RECORD_FILE};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

const SIZE: [f32; 2] = [760.0, 560.0];
const TITLE_H: f32 = 40.0;
const BRAND_W: f32 = 300.0;
const LICENCE: &str = include_str!("../../../LICENSE.md");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Welcome,
    Licence,
    Working,
    Done,
    Failed,
    Uninstall,
    Uninstalled,
}

/// What runs on the task thread.
#[derive(Clone, Debug)]
enum Job {
    /// The options, and whether they are a fresh choice (not an install kept as it was).
    Install(Options, bool),
    Update(PathBuf),
    /// The folder, and whether its settings and downloads go too.
    Uninstall(PathBuf, bool),
}

/// Opens the window; its exit code once closed (0 when the task succeeded). An error when it cannot open at all.
pub fn run(args: Args, payload: Option<&'static [u8]>) -> anyhow::Result<i32> {
    let title = match args.mode {
        Mode::Install => "Orbiters - N4N0 setup",
        Mode::Update => "Updating Orbiters - N4N0",
        Mode::Uninstall => "Uninstall Orbiters - N4N0",
    };
    let code = Arc::new(AtomicI32::new(1));
    let out = code.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id("orbiters-setup")
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size(SIZE)
            .with_icon(window_icon()),
        renderer: eframe::Renderer::Wgpu,
        // The least power-hungry GPU, the processor's integrated one where there is one, as the app's window: an update
        // may install while VRChat plays on the graphics card.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetupCreateNew {
                power_preference: eframe::wgpu::PowerPreference::LowPower,
                ..eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle()
            }
            .into(),
            ..Default::default()
        },
        centered: true,
        // `orbiters_ui::install` brings its own fallback fonts.
        system_font_fallback: false,
        ..Default::default()
    };
    eframe::run_native(title, options, Box::new(move |cc| Ok(Box::new(Setup::new(cc, args, payload, out))))).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(code.load(Ordering::Relaxed))
}

struct Setup {
    args: Args,
    payload: Option<&'static [u8]>,
    index: Option<Index>,
    page: Page,
    page_since: f64,
    /// Bumped on every page, so its entrance plays.
    visit: u64,
    started: Option<f64>,
    glitch: Option<(f64, f32)>,
    logo_hovered: bool,
    dir: PathBuf,
    /// The install already there (it is updated where it is).
    existing: Option<InstallRecord>,
    desktop_shortcut: bool,
    start_with_windows: bool,
    start_with_steamvr: bool,
    launch: bool,
    remove_data: bool,
    picker: Option<mpsc::Receiver<Option<PathBuf>>>,
    task: Option<Arc<Progress>>,
    job: Option<Job>,
    error: Option<String>,
    /// When an update finished (the app starts again a moment later).
    finished_at: Option<f64>,
    code: Arc<AtomicI32>,
}

impl Setup {
    fn new(cc: &eframe::CreationContext<'_>, args: Args, payload: Option<&'static [u8]>, code: Arc<AtomicI32>) -> Self {
        orbiters_ui::install(&cc.egui_ctx);
        if let Some(rs) = &cc.wgpu_render_state {
            orbiters_ui::install_wgpu(&rs.device, rs.target_format, 1, &mut rs.renderer.write().callback_resources);
        }
        round_corners(cc);
        Self::build(args, payload, code)
    }

    fn build(args: Args, payload: Option<&'static [u8]>, code: Arc<AtomicI32>) -> Self {
        let index = payload.and_then(|p| orbiters_update::payload::read_index(p).ok());
        let dir = match args.mode {
            Mode::Install => crate::target_dir(&args),
            Mode::Update | Mode::Uninstall => crate::install_dir(&args).unwrap_or_else(orbiters_update::default_install_dir),
        };
        let existing = InstallRecord::read(&dir);
        let mut setup = Self {
            page: match args.mode {
                Mode::Install => Page::Welcome,
                Mode::Update => Page::Working,
                Mode::Uninstall => Page::Uninstall,
            },
            args,
            payload,
            index,
            page_since: 0.0,
            visit: 0,
            started: None,
            glitch: None,
            logo_hovered: false,
            desktop_shortcut: existing.as_ref().is_none_or(|r| r.desktop_shortcut),
            start_with_windows: existing.as_ref().is_some_and(|r| r.start_with_windows),
            start_with_steamvr: existing.as_ref().is_some_and(|r| r.start_with_steamvr),
            dir,
            existing,
            launch: true,
            remove_data: false,
            picker: None,
            task: None,
            job: None,
            error: None,
            finished_at: None,
            code,
        };
        match setup.args.mode {
            Mode::Update => setup.start(Job::Update(setup.dir.clone())),
            Mode::Uninstall if setup.existing.is_none() => {
                setup.error = Some(format!("Orbiters - N4N0 is not installed in {}.", setup.dir.display()));
                setup.page = Page::Failed;
            }
            _ => {}
        }
        setup
    }

    fn version(&self) -> &str {
        self.index.as_ref().map(|i| i.version.as_str()).unwrap_or(env!("CARGO_PKG_VERSION"))
    }

    fn go(&mut self, page: Page, now: f64) {
        self.page = page;
        self.page_since = now;
        self.visit += 1;
    }

    fn busy(&self) -> bool {
        self.task.is_some()
    }

    fn start(&mut self, job: Job) {
        let progress = Arc::new(Progress::default());
        let task = progress.clone();
        let payload = self.payload;
        let run = job.clone();
        std::thread::Builder::new()
            .name("setup".into())
            .spawn(move || {
                let result = match run {
                    Job::Install(options, fresh) => match payload {
                        Some(bytes) => install::install(bytes, &options, &task, false, fresh),
                        None => Err(anyhow::anyhow!("This setup holds no programs to install.")),
                    },
                    Job::Update(dir) => match payload {
                        Some(bytes) => install::update(bytes, &dir, &task, false).map(drop),
                        None => Err(anyhow::anyhow!("This setup holds no programs to install.")),
                    },
                    Job::Uninstall(dir, remove_data) => install::uninstall(&dir, remove_data, &task, false),
                };
                task.finish(result);
            })
            .expect("a thread for the setup");
        self.task = Some(progress);
        self.job = Some(job);
        self.error = None;
        self.page = Page::Working;
        self.visit += 1;
        self.page_since = f64::NEG_INFINITY;
    }

    /// The running task's outcome, and the folder picker's answer.
    fn poll(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(rx) = &self.picker {
            if let Ok(choice) = rx.try_recv() {
                self.picker = None;
                if let Some(dir) = choice {
                    // A folder of its own inside the one chosen, unless that already is one.
                    let own = dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(DISPLAY_NAME)) || dir.join(RECORD_FILE).is_file();
                    self.dir = if own { dir } else { dir.join(DISPLAY_NAME) };
                    self.existing = InstallRecord::read(&self.dir);
                }
            }
        }
        if let Some(task) = &self.task {
            match task.status().done {
                None => ctx.request_repaint_after(Duration::from_millis(50)),
                Some(Ok(())) => {
                    self.task = None;
                    self.code.store(0, Ordering::Relaxed);
                    let uninstalled = matches!(self.job, Some(Job::Uninstall(..)));
                    self.go(if uninstalled { Page::Uninstalled } else { Page::Done }, now);
                    if self.args.mode == Mode::Update {
                        self.finished_at = Some(now);
                    }
                }
                Some(Err(e)) => {
                    self.task = None;
                    self.code.store(1, Ordering::Relaxed);
                    self.error = Some(e);
                    self.go(Page::Failed, now);
                }
            }
        }
        // An update done: the app starts again, the window goes.
        if let Some(at) = self.finished_at {
            if now - at >= 1.1 {
                self.finished_at = None;
                if self.args.relaunch {
                    crate::relaunch(&self.dir, self.args.hidden);
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn close(&self, ctx: &egui::Context) {
        if !self.busy() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let start = self.dir.ancestors().find(|d| d.is_dir()).map(PathBuf::from).unwrap_or_default();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let chosen = rfd::FileDialog::new().set_title("Where to install Orbiters - N4N0").set_directory(start).pick_folder();
            let _ = tx.send(chosen);
            ctx.request_repaint();
        });
        self.picker = Some(rx);
    }

    fn title_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let id = Id::unique("setup-title-bar");
        let bar = ui.interact(rect, id, Sense::click_and_drag());
        if bar.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let title = match self.args.mode {
            Mode::Install => "Orbiters - N4N0 · Setup",
            Mode::Update => "Orbiters - N4N0 · Update",
            Mode::Uninstall => "Orbiters - N4N0 · Uninstall",
        };
        widgets::text(
            ui.painter(),
            pos2(rect.left() + 16.0, rect.center().y),
            Align2::LEFT_CENTER,
            &Txt::new(title, 12.0).weight(weight::SEMIBOLD).tracking(0.025).color(theme::FG_2),
        );
        let busy = self.busy();
        let buttons = [(icons::MINUS, "Minimize", 15.0, false), (icons::X, if busy { "Wait for it to finish" } else { "Close" }, 16.0, true)];
        for (i, (glyph, label, size, danger)) in buttons.into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(rect.right() - 4.0 - 44.0 * (2 - i) as f32, rect.top() + 4.0), vec2(44.0, 32.0));
            let enabled = !(danger && busy);
            let resp = ui.interact(b, id.with(i), Sense::click()).on_hover_text(label);
            let hover = anim::toggle(&ctx, id.with(("hover", i)), enabled && resp.hovered(), Spring::SNAPPY);
            let press = anim::toggle(&ctx, id.with(("press", i)), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let br = Rect::from_center_size(b.center(), b.size() * (1.0 - 0.14 * press));
            let (bg, fg) = if danger {
                (theme::alpha(theme::REC, 0.85 * hover), theme::mix(theme::FG_3, Color32::WHITE, hover))
            } else {
                (theme::white(0.08 * hover), theme::mix(theme::FG_3, theme::FG, hover))
            };
            let p = ui.painter();
            p.rect_filled(br, theme::radius(8.0), bg);
            widgets::icon(p, br.center(), glyph, size, if enabled { fg } else { theme::FG_4 });
            if resp.clicked() && enabled {
                if danger {
                    self.close(&ctx);
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }
            }
        }
    }

    /// The mark drawing itself in, the name, the version: as the app's splash.
    fn brand(&mut self, ui: &mut Ui, rect: Rect, started: f64, now: f64) {
        let ctx = ui.ctx().clone();
        let resp = ui.interact(rect, Id::unique("setup-brand"), Sense::click_and_drag());
        if resp.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let center = pos2(rect.center().x, rect.top() + rect.height() * 0.40);
        let logo_rect = Rect::from_center_size(center, logo::size_for_width(128.0));
        let hovered = ui.rect_contains_pointer(logo_rect.expand(12.0));
        if hovered && !self.logo_hovered {
            self.glitch = Some((now, 0.8));
        }
        self.logo_hovered = hovered;
        let glitch = *self.glitch.get_or_insert((started + 0.3, 1.2));
        let k = ((now - started) / 0.35).clamp(0.0, 1.0) as f32;
        let mut lp = ui.painter().clone();
        lp.multiply_opacity(k);
        logo::paint(&lp, logo_rect, logo::Look { intro_start: Some(started), glitch: Some(glitch) });
        if now - started < 2.0 {
            ctx.request_repaint();
        }

        let t = anim::ease::segment((now - started) as f32, 0.75, 0.6, anim::ease::out_expo);
        let mut p = ui.painter().clone();
        p.multiply_opacity(t);
        let y = logo_rect.bottom() + 28.0 + 12.0 * (1.0 - t);
        widgets::text(&p, pos2(rect.center().x, y + 12.0), Align2::CENTER_CENTER, &Txt::new("Orbiters", 26.0).weight(weight::SEMIBOLD).tracking(-0.01));
        let galley = p.layout_job(Txt::new("N4N0", 12.5).weight(weight::MEDIUM).tracking(0.32).color(theme::FG_3).job(f32::INFINITY));
        p.galley(pos2(rect.center().x - galley.size().x * 0.5 + 0.16 * 12.5, y + 38.0 - galley.size().y * 0.5), galley, theme::FG);

        let version = Txt::new(format!("Version {}", self.version()), 11.5).weight(weight::MEDIUM).color(theme::FG_3);
        let size = widgets::text_size(ui, &version);
        let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 44.0), vec2(size.x + 24.0, 26.0));
        p.rect_filled(pill, theme::radius(13.0), theme::white(0.06));
        widgets::text(&p, pill.center(), Align2::CENTER_CENTER, &version);
        p.vline(rect.right(), (rect.top() + 24.0)..=(rect.bottom() - 24.0), egui::Stroke::new(1.0, theme::white(0.05)));
    }

    /// The title of the welcome page, its primary button and what it says of an install already there.
    fn action(&self) -> (&'static str, &'static str, Option<String>, bool) {
        let new = self.version();
        match &self.existing {
            None => ("Install Orbiters - N4N0", "Install", None, false),
            Some(r) if orbiters_update::is_newer(new, &r.version) => {
                ("Update Orbiters - N4N0", "Update", Some(format!("Version {} is installed: this updates it to {new}, where it is.", r.version)), false)
            }
            Some(r) if r.version == new => {
                ("Reinstall Orbiters - N4N0", "Reinstall", Some(format!("Version {new} is already installed: this installs it again.")), false)
            }
            Some(r) => {
                ("Install an older version", "Install anyway", Some(format!("Version {} is installed, newer than this setup's {new}.", r.version)), true)
            }
        }
    }

    fn welcome(&mut self, ui: &mut Ui, r: Rect, now: f64) {
        let (title, verb, note, warn) = self.action();
        let (visit, since) = (self.visit, self.page_since);
        let sub = note.unwrap_or_else(|| "N4N0 and its menu for VRChat: fast walk, YouTube, calls, 3D pictures, music and the light.".into());
        block(ui, visit, since, 0, r, |ui| heading(ui, title, Some(&sub), if warn { theme::WARN } else { theme::FG_3 }));

        let locked = self.existing.is_some();
        let dir_text = short_path(&self.dir.display().to_string(), 64);
        let mut change = false;
        let (desktop, autostart, steamvr) = (&mut self.desktop_shortcut, &mut self.start_with_windows, &mut self.start_with_steamvr);
        let card_area = Rect::from_min_max(pos2(r.left(), r.top() + 92.0), r.max);
        block(ui, visit, since, 1, card_area, |ui| {
            card(ui, |ui| {
                widgets::field(ui, "Install location", Some(&dir_text), 104.0, |ui| {
                    let button = Button::new("Change").icon(icons::FOLDER).size(Size::Sm).enabled(!locked);
                    let button = if locked { button.hint("An install is updated where it is: uninstall it to move it.") } else { button };
                    change = button.show(ui).clicked();
                });
                widgets::separator(ui);
                widgets::field(ui, "Desktop shortcut", None, 48.0, |ui| {
                    widgets::toggle(ui, "desktop-shortcut", desktop);
                });
                widgets::separator(ui);
                widgets::field(ui, "Start with Windows", Some("Waits in the tray, ready when VRChat starts."), 48.0, |ui| {
                    widgets::toggle(ui, "start-with-windows", autostart);
                });
                widgets::separator(ui);
                widgets::field(ui, "Start with SteamVR", Some("Opens when SteamVR starts, in its dashboard."), 48.0, |ui| {
                    widgets::toggle(ui, "start-with-steamvr", steamvr);
                });
            });
        });
        if change && self.picker.is_none() {
            self.pick_folder(ui.ctx());
        }

        let needs = self.index.as_ref().map(|i| format!("Needs {} of space. ", format::bytes(i.total() as f64))).unwrap_or_default();
        let note_rect = Rect::from_min_max(pos2(r.left(), r.bottom() - 74.0), pos2(r.right(), r.bottom() - 52.0));
        block(ui, visit, since, 2, note_rect, |ui| {
            paragraph(ui, &format!("{needs}By installing, you accept the licence."), 12.0, theme::FG_4);
        });

        let mut install = false;
        let mut licence = false;
        let mut cancel = false;
        footer(ui, visit, since, 3, r, |ui, row| {
            licence = left_button(ui, row, Button::new("Licence").icon(icons::SCROLL).variant(Variant::Ghost));
            let primary = Button::new(verb).icon(icons::DOWNLOAD_SIMPLE).variant(Variant::Primary);
            let w = primary.desired_width(ui);
            install = primary.show_at(ui, Rect::from_min_size(pos2(row.right() - w, row.top()), vec2(w, 40.0))).clicked();
            let secondary = Button::new("Cancel");
            let sw = secondary.desired_width(ui);
            cancel = secondary.show_at(ui, Rect::from_min_size(pos2(row.right() - w - 10.0 - sw, row.top()), vec2(sw, 40.0))).clicked();
        });
        install |= ui.input(|i| i.key_pressed(egui::Key::Enter));
        if licence {
            self.go(Page::Licence, now);
        } else if cancel {
            self.close(ui.ctx());
        } else if install && self.picker.is_none() {
            let options = Options {
                dir: self.dir.clone(),
                desktop_shortcut: self.desktop_shortcut,
                start_with_windows: self.start_with_windows,
                start_with_steamvr: self.start_with_steamvr,
                channel: self.existing.as_ref().map(|r| r.channel.clone()).unwrap_or_else(|| orbiters_update::STABLE.into()),
            };
            self.start(Job::Install(options, true));
        }
    }

    fn licence(&mut self, ui: &mut Ui, r: Rect, now: f64) {
        let (visit, since) = (self.visit, self.page_since);
        block(ui, visit, since, 0, r, |ui| heading(ui, "Licence", Some("Free to use, under these terms."), theme::FG_3));
        let body = Rect::from_min_max(pos2(r.left(), r.top() + 84.0), pos2(r.right(), r.bottom() - 60.0));
        block(ui, visit, since, 1, body, |ui| {
            egui::ScrollArea::vertical().id_salt("licence").auto_shrink([false, false]).max_height(body.height()).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                licence_text(ui);
            });
        });
        let mut back = false;
        footer(ui, visit, since, 2, r, |ui, row| {
            let b = Button::new("Back").icon(icons::ARROW_LEFT).variant(Variant::Primary);
            let w = b.desired_width(ui);
            back = b.show_at(ui, Rect::from_min_size(pos2(row.right() - w, row.top()), vec2(w, 40.0))).clicked();
        });
        if back || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.go(Page::Welcome, now);
        }
    }

    fn working(&mut self, ui: &mut Ui, r: Rect) {
        let Some(task) = self.task.clone() else { return };
        let status = task.status();
        let (visit, since) = (self.visit, self.page_since);
        let (title, sub) = match &self.job {
            Some(Job::Update(_)) => (
                "Updating Orbiters - N4N0".to_string(),
                match &self.existing {
                    Some(old) => format!("From version {} to {}. It opens again once done.", old.version, self.version()),
                    None => format!("To version {}.", self.version()),
                },
            ),
            Some(Job::Uninstall(..)) => ("Uninstalling Orbiters - N4N0".into(), "Its files are removed; your sessions stay.".into()),
            _ => ("Installing Orbiters - N4N0".into(), format!("Version {}, in {}.", self.version(), self.dir.display())),
        };
        block(ui, visit, since, 0, r, |ui| heading(ui, &title, Some(&sub), theme::FG_3));
        let area = Rect::from_min_max(pos2(r.left(), r.top() + 104.0), r.max);
        block(ui, visit, since, 1, area, |ui| {
            let w = ui.available_width();
            let (row, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
            let percent = Txt::new(format!("{:.0} %", status.fraction * 100.0), 12.5).weight(weight::MEDIUM).color(theme::FG_2);
            let pw = widgets::text_size(ui, &percent).x;
            widgets::text(ui.painter(), pos2(row.right(), row.center().y), Align2::RIGHT_CENTER, &percent);
            widgets::text_truncated(
                ui.painter(),
                pos2(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                &Txt::new(&status.label, 13.0).color(theme::FG_2),
                w - pw - 16.0,
            );
            ui.add_space(10.0);
            bar(ui, Id::unique(("setup-bar", visit)), status.fraction);
        });
        if status.stuck {
            let area = Rect::from_min_max(pos2(r.left(), r.top() + 176.0), r.max);
            let can_cancel = !matches!(self.job, Some(Job::Update(_)));
            let mut force = false;
            let mut cancel = false;
            block(ui, visit, since, 2, area, |ui| {
                card(ui, |ui| {
                    ui.add_space(12.0);
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                    widgets::icon(ui.painter(), pos2(row.left() + 10.0, row.center().y), icons::WARNING, 18.0, theme::WARN);
                    widgets::text(
                        ui.painter(),
                        pos2(row.left() + 30.0, row.center().y),
                        Align2::LEFT_CENTER,
                        &Txt::new("Orbiters - N4N0 is still closing", 14.0).weight(weight::SEMIBOLD),
                    );
                    ui.add_space(6.0);
                    paragraph(ui, "It finishes saving a recording before it quits. Close it now, and what it has not saved yet is lost.", 12.5, theme::FG_3);
                    ui.add_space(12.0);
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                    let b = Button::new("Close it now").icon(icons::X_CIRCLE).variant(Variant::Danger).size(Size::Sm);
                    let w = b.desired_width(ui);
                    force = b.show_at(ui, Rect::from_min_size(row.min, vec2(w, 32.0))).clicked();
                    if can_cancel {
                        let c = Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm);
                        let cw = c.desired_width(ui);
                        cancel = c.show_at(ui, Rect::from_min_size(pos2(row.left() + w + 8.0, row.top()), vec2(cw, 32.0))).clicked();
                    }
                    ui.add_space(14.0);
                });
            });
            if force {
                task.force();
            }
            if cancel {
                task.cancel();
            }
        }
    }

    fn done(&mut self, ui: &mut Ui, r: Rect) {
        let (visit, since) = (self.visit, self.page_since);
        let updating = self.args.mode == Mode::Update;
        let (title, sub) = if updating {
            (
                format!("Updated to {}", self.version()),
                if self.args.relaunch { "Orbiters - N4N0 is opening again…".to_string() } else { format!("Version {} is in place.", self.version()) },
            )
        } else {
            let where_ = if self.desktop_shortcut { "in the Start menu and on your desktop" } else { "in the Start menu" };
            ("Orbiters - N4N0 is installed".to_string(), format!("Version {}. Find it {where_}.", self.version()))
        };
        block(ui, visit, since, 0, r, |ui| badge(ui, icons::CHECK, theme::ACCENT));
        let text = Rect::from_min_max(pos2(r.left(), r.top() + 84.0), r.max);
        block(ui, visit, since, 1, text, |ui| heading(ui, &title, Some(&sub), theme::FG_3));
        if updating && self.args.relaunch {
            return;
        }
        let launch = &mut self.launch;
        let card_area = Rect::from_min_max(pos2(r.left(), r.top() + 168.0), r.max);
        block(ui, visit, since, 2, card_area, |ui| {
            card(ui, |ui| {
                widgets::field(ui, "Open Orbiters - N4N0 now", None, 48.0, |ui| {
                    widgets::toggle(ui, "launch", launch);
                });
            });
        });
        let mut finish = false;
        footer(ui, visit, since, 3, r, |ui, row| {
            let b = Button::new("Finish").icon(icons::CHECK).variant(Variant::Primary);
            let w = b.desired_width(ui);
            finish = b.show_at(ui, Rect::from_min_size(pos2(row.right() - w, row.top()), vec2(w, 40.0))).clicked();
        });
        if finish || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if self.launch {
                crate::relaunch(&self.dir, false);
            }
            self.close(ui.ctx());
        }
    }

    fn failed(&mut self, ui: &mut Ui, r: Rect) {
        let (visit, since) = (self.visit, self.page_since);
        let title = match (&self.job, self.args.mode) {
            (Some(Job::Update(_)), _) | (None, Mode::Update) => "The update did not finish",
            (Some(Job::Uninstall(..)), _) | (None, Mode::Uninstall) => "The uninstall did not finish",
            _ => "The install did not finish",
        };
        let error = self.error.clone().unwrap_or_default();
        block(ui, visit, since, 0, r, |ui| badge(ui, icons::WARNING, theme::DANGER));
        let text = Rect::from_min_max(pos2(r.left(), r.top() + 84.0), r.max);
        block(ui, visit, since, 1, text, |ui| {
            heading(ui, title, None, theme::FG_3);
            ui.add_space(14.0);
            card(ui, |ui| {
                ui.add_space(12.0);
                paragraph(ui, &error, 12.5, theme::FG_2);
                ui.add_space(14.0);
            });
        });
        let retry = self.job.clone();
        let reopen = self.args.mode == Mode::Update && self.existing.is_some();
        let (mut again, mut close, mut open) = (false, false, false);
        footer(ui, visit, since, 2, r, |ui, row| {
            let mut right = row.right();
            if retry.is_some() {
                let b = Button::new("Try again").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary);
                let w = b.desired_width(ui);
                again = b.show_at(ui, Rect::from_min_size(pos2(right - w, row.top()), vec2(w, 40.0))).clicked();
                right -= w + 10.0;
            }
            let c = Button::new("Close");
            let w = c.desired_width(ui);
            close = c.show_at(ui, Rect::from_min_size(pos2(right - w, row.top()), vec2(w, 40.0))).clicked();
            if reopen {
                open = left_button(ui, row, Button::new("Open Orbiters - N4N0").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost));
            }
        });
        if again {
            if let Some(job) = retry {
                self.start(job);
            }
        } else if open {
            crate::relaunch(&self.dir, self.args.hidden);
            self.close(ui.ctx());
        } else if close {
            self.close(ui.ctx());
        }
    }

    fn uninstall(&mut self, ui: &mut Ui, r: Rect) {
        let (visit, since) = (self.visit, self.page_since);
        let version = self.existing.as_ref().map(|e| e.version.clone()).unwrap_or_default();
        let sub = format!("Version {version}, in {}.", self.dir.display());
        block(ui, visit, since, 0, r, |ui| heading(ui, "Uninstall Orbiters - N4N0?", Some(&sub), theme::FG_3));
        let remove = &mut self.remove_data;
        let card_area = Rect::from_min_max(pos2(r.left(), r.top() + 92.0), r.max);
        block(ui, visit, since, 1, card_area, |ui| {
            card(ui, |ui| {
                widgets::field(
                    ui,
                    "Also remove its settings and downloads",
                    Some("Settings, caches, downloaded updates and N4N0's speech models. Your recorded sessions are kept either way."),
                    48.0,
                    |ui| {
                        widgets::toggle(ui, "remove-data", remove);
                    },
                );
            });
        });
        let (mut go, mut cancel) = (false, false);
        footer(ui, visit, since, 2, r, |ui, row| {
            let b = Button::new("Uninstall").icon(icons::TRASH).variant(Variant::Danger);
            let w = b.desired_width(ui);
            go = b.show_at(ui, Rect::from_min_size(pos2(row.right() - w, row.top()), vec2(w, 40.0))).clicked();
            let c = Button::new("Cancel");
            let cw = c.desired_width(ui);
            cancel = c.show_at(ui, Rect::from_min_size(pos2(row.right() - w - 10.0 - cw, row.top()), vec2(cw, 40.0))).clicked();
        });
        if go {
            self.start(Job::Uninstall(self.dir.clone(), self.remove_data));
        } else if cancel || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close(ui.ctx());
        }
    }

    fn uninstalled(&mut self, ui: &mut Ui, r: Rect) {
        let (visit, since) = (self.visit, self.page_since);
        let sub = if self.remove_data {
            "Your recorded sessions are still where they were."
        } else {
            "Your recorded sessions and its settings are still there, should you install it again."
        };
        block(ui, visit, since, 0, r, |ui| badge(ui, icons::CHECK, theme::ACCENT));
        let text = Rect::from_min_max(pos2(r.left(), r.top() + 84.0), r.max);
        block(ui, visit, since, 1, text, |ui| heading(ui, "Orbiters - N4N0 is uninstalled", Some(sub), theme::FG_3));
        let mut close = false;
        footer(ui, visit, since, 2, r, |ui, row| {
            let b = Button::new("Close").variant(Variant::Primary);
            let w = b.desired_width(ui);
            close = b.show_at(ui, Rect::from_min_size(pos2(row.right() - w, row.top()), vec2(w, 40.0))).clicked();
        });
        if close || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)) {
            self.close(ui.ctx());
        }
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        crate::done_relocated(&self.args);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::INK_950.to_normalized_gamma_f32()
    }
}

impl Setup {
    /// One frame of the window.
    fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let started = *self.started.get_or_insert(now);
        if self.page_since == 0.0 || self.page_since == f64::NEG_INFINITY {
            self.page_since = now;
        }
        anim::set_live(&ctx, ctx.input(|i| i.viewport().focused.unwrap_or(true)));
        self.poll(&ctx, now);

        let full = ui.max_rect();
        glow::paint(ui.painter(), full, if self.page == Page::Working { Mood::Importing } else { Mood::Idle });
        self.brand(ui, Rect::from_min_max(pos2(full.left(), full.top() + TITLE_H), pos2(full.left() + BRAND_W, full.bottom())), started, now);
        let right = Rect::from_min_max(pos2(full.left() + BRAND_W + 36.0, full.top() + TITLE_H + 24.0), pos2(full.right() - 36.0, full.bottom() - 28.0));
        match self.page {
            Page::Welcome => self.welcome(ui, right, now),
            Page::Licence => self.licence(ui, right, now),
            Page::Working => self.working(ui, right),
            Page::Done => self.done(ui, right),
            Page::Failed => self.failed(ui, right),
            Page::Uninstall => self.uninstall(ui, right),
            Page::Uninstalled => self.uninstalled(ui, right),
        }
        self.title_bar(ui, Rect::from_min_size(full.min, vec2(full.width(), TITLE_H)));
        // Alt+F4 and the taskbar's Close wait for a running task too.
        if self.busy() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
    }
}

/// A part of the page that enters after the ones before it (`index`), laid out top-down in `rect`.
fn block(ui: &mut Ui, visit: u64, since: f64, index: usize, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let (alpha, dy) = widgets::entrance(ui, Id::unique(("setup-enter", visit, index)), since, index);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.translate(vec2(0.0, dy))).layout(Layout::top_down(Align::Min)).id_salt(("setup-block", index)));
    child.multiply_opacity(alpha);
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    add(&mut child);
}

/// The buttons along the bottom of the page: `add` places them in the row it gets.
fn footer(ui: &mut Ui, visit: u64, since: f64, index: usize, page: Rect, add: impl FnOnce(&mut Ui, Rect)) {
    let row = Rect::from_min_max(pos2(page.left(), page.bottom() - 40.0), page.max);
    block(ui, visit, since, index, row, |ui| {
        let row = Rect::from_min_size(ui.max_rect().min, vec2(row.width(), 40.0));
        add(ui, row);
    });
}

/// A button at the left end of a footer row; whether it was clicked.
fn left_button(ui: &mut Ui, row: Rect, button: Button<'_>) -> bool {
    let w = button.desired_width(ui);
    button.show_at(ui, Rect::from_min_size(row.min, vec2(w, 40.0))).clicked()
}

/// The page's title, and what it says under it.
fn heading(ui: &mut Ui, title: &str, sub: Option<&str>, sub_color: Color32) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(title, 22.0).weight(weight::SEMIBOLD).tracking(-0.01).job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, theme::FG);
    if let Some(sub) = sub {
        ui.add_space(6.0);
        paragraph(ui, sub, 13.0, sub_color);
    }
}

/// Wrapped text across the width.
fn paragraph(ui: &mut Ui, text: &str, size: f32, color: Color32) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, size).color(color).line_height((size * 1.45).round()).job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// A card (the app's settings sections) around what `add` lays out.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let bg = ui.painter().add(Shape::Noop);
    let top = ui.cursor().min;
    let w = ui.available_width();
    let inner = Rect::from_min_size(top + vec2(20.0, 4.0), vec2(w - 40.0, 10_000.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt(("setup-card", top.y as i32)));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    add(&mut child);
    let h = child.min_rect().height() + 8.0;
    let rect = Rect::from_min_size(top, vec2(w, h));
    let fill = theme::surface().gamma_multiply(ui.opacity());
    ui.painter().set(bg, Shape::mesh(paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), fill, 1.0 / ui.ctx().pixels_per_point())));
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
}

/// A round badge with an icon that springs in (done, failed).
fn badge(ui: &mut Ui, glyph: &str, color: Color32) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let k = anim::spring_from(ui.ctx(), ui.scope_id().with(("badge", glyph)), [0.5], [1.0], Spring::BOUNCY)[0];
    let center = pos2(row.left() + 32.0, row.center().y);
    let p = ui.painter();
    p.circle_filled(center, 30.0 * k, theme::alpha(color, 0.14));
    widgets::icon(p, center, glyph, 28.0 * k, color);
}

/// The task's progress: a green bar whose fill springs to the fraction, with a sheen moving along it while it runs.
fn bar(ui: &mut Ui, id: Id, fraction: f32) {
    let h = 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(h * 0.5), theme::white(0.07));
    let f = anim::spring_from(ui.ctx(), id, [0.0], [fraction.clamp(0.0, 1.0)], Spring::SOFT)[0].clamp(0.0, 1.0);
    let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(h), h));
    p.rect_filled(fill, theme::radius(h * 0.5), theme::ACCENT);
    if f < 0.999 && anim::is_live(ui.ctx()) {
        let t = (anim::clock(ui.ctx(), 60.0) * 0.6).fract() as f32;
        let x = fill.left() + (fill.width() + 80.0) * t - 40.0;
        let sheen = Rect::from_min_max(pos2((x - 40.0).max(fill.left()), fill.top()), pos2((x + 40.0).min(fill.right()), fill.bottom()));
        if sheen.width() > 0.0 {
            p.rect_filled(sheen, theme::radius(h * 0.5), theme::white(0.18));
        }
    }
}

/// The licence (LICENSE.md) as text: headings, paragraphs, list items; links show their text.
fn licence_text(ui: &mut Ui) {
    for line in LICENCE.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            ui.add_space(8.0);
        } else if let Some(h) = line.strip_prefix("# ") {
            paragraph_styled(ui, Txt::new(unlink(h), 16.0).weight(weight::SEMIBOLD));
            ui.add_space(4.0);
        } else if let Some(h) = line.strip_prefix("## ") {
            ui.add_space(6.0);
            paragraph_styled(ui, Txt::new(unlink(h), 13.5).weight(weight::SEMIBOLD));
            ui.add_space(2.0);
        } else if let Some(item) = line.strip_prefix("- ") {
            paragraph_styled(ui, Txt::new(format!("•  {}", unlink(item)), 12.5).color(theme::FG_2).line_height(18.0));
            ui.add_space(3.0);
        } else {
            paragraph_styled(ui, Txt::new(unlink(line), 12.5).color(theme::FG_2).line_height(18.0));
        }
    }
}

fn paragraph_styled(ui: &mut Ui, txt: Txt) {
    let w = ui.available_width() - 12.0;
    let color = txt.color;
    let g = ui.painter().layout_job(txt.job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// A folder short enough for two lines: its drive and first folders, an ellipsis, and its last two folders (where
/// it is, and its own name).
fn short_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('\\').collect();
    if parts.len() > 5 {
        let short = format!("{}\\…\\{}", parts[..3].join("\\"), parts[parts.len() - 2..].join("\\"));
        if short.chars().count() <= max {
            return short;
        }
    }
    let chars: Vec<char> = path.chars().collect();
    let keep = max.saturating_sub(1) / 2;
    format!("{}…{}", chars[..keep].iter().collect::<String>(), chars[chars.len() - keep..].iter().collect::<String>())
}

/// Markdown links (`[text](address)`) as their text.
fn unlink(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Windows 11 rounds the corners of a frameless window only when asked.
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

/// The app's icon (taskbar, Alt+Tab).
fn window_icon() -> egui::IconData {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../../orbiters-desktop/assets/128x128@2x.png")));
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

/// `--screenshots <folder>`: every page rendered offscreen with the window's own renderer, for review.
pub fn screenshots(dir: &std::path::Path, payload: Option<&'static [u8]>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use egui::{Pos2, RawInput, ViewportId, ViewportInfo};
    use orbiters_ui::offscreen::{save_png, Offscreen};
    std::fs::create_dir_all(dir)?;
    let mut gpu = Offscreen::new()?;
    let older = InstallRecord {
        product: orbiters_update::PRODUCT.into(),
        version: "0.0.9".into(),
        channel: orbiters_update::STABLE.into(),
        files: Vec::new(),
        desktop_shortcut: true,
        start_with_windows: false,
        start_with_steamvr: false,
        installed: String::new(),
    };
    fn working(label: &str, fraction: f32, stuck: bool) -> Arc<Progress> {
        let progress = Arc::new(Progress::default());
        progress.show(fraction, label, stuck);
        progress
    }
    type Prepare = Box<dyn Fn(&mut Setup)>;
    let shots: Vec<(&str, Prepare)> = vec![
        ("welcome", Box::new(|_| {})),
        ("welcome-update", Box::new(move |s| s.existing = Some(older.clone()))),
        ("licence", Box::new(|s| s.page = Page::Licence)),
        (
            "installing",
            Box::new(move |s| {
                s.page = Page::Working;
                s.job = Some(Job::Install(
                    Options { dir: s.dir.clone(), desktop_shortcut: true, start_with_windows: false, start_with_steamvr: false, channel: "stable".into() },
                    true,
                ));
                s.task = Some(working("Unpacking orbiters-desktop.exe", 0.42, false));
            }),
        ),
        (
            "closing",
            Box::new(move |s| {
                s.page = Page::Working;
                s.job = Some(Job::Install(
                    Options { dir: s.dir.clone(), desktop_shortcut: true, start_with_windows: false, start_with_steamvr: false, channel: "stable".into() },
                    true,
                ));
                s.task = Some(working("Closing Orbiters - N4N0…", 0.8, true));
            }),
        ),
        ("done", Box::new(|s| s.page = Page::Done)),
        (
            "failed",
            Box::new(|s| {
                s.page = Page::Failed;
                s.error = Some(r"C:\Users\you\AppData\Local\Programs\Orbiters - N4N0\orbiters-voice.exe is in use by another program: The process cannot access the file because it is being used by another process. (os error 32): The previous version was put back".into());
                s.job = Some(Job::Update(s.dir.clone()));
            }),
        ),
        (
            "updating",
            Box::new(|s| {
                s.args.mode = Mode::Update;
                s.page = Page::Working;
                s.existing = Some(InstallRecord {
                    version: "0.0.9".into(),
                    ..s.existing.clone().unwrap_or_else(|| InstallRecord {
                        product: orbiters_update::PRODUCT.into(),
                        version: String::new(),
                        channel: "stable".into(),
                        files: Vec::new(),
                        desktop_shortcut: true,
                        start_with_windows: false,
                        start_with_steamvr: false,
                        installed: String::new(),
                    })
                });
                s.job = Some(Job::Update(s.dir.clone()));
                s.task = Some(working("Putting the new version in place…", 0.86, false));
            }),
        ),
        (
            "uninstall",
            Box::new(|s| {
                s.args.mode = Mode::Uninstall;
                s.page = Page::Uninstall;
                s.existing = Some(InstallRecord {
                    product: orbiters_update::PRODUCT.into(),
                    version: "0.1.0".into(),
                    channel: "stable".into(),
                    files: Vec::new(),
                    desktop_shortcut: true,
                    start_with_windows: false,
                    start_with_steamvr: false,
                    installed: String::new(),
                });
            }),
        ),
        (
            "uninstalled",
            Box::new(|s| {
                s.args.mode = Mode::Uninstall;
                s.page = Page::Uninstalled;
            }),
        ),
    ];
    let args = Args { mode: Mode::Install, silent: false, dir: Some(orbiters_update::default_install_dir()), relaunch: true, hidden: false, relocated: false };
    for (name, prepare) in shots {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut setup = Setup::build(args.clone(), payload, Arc::new(AtomicI32::new(1)));
        setup.existing = None;
        prepare(&mut setup);
        let frames = 160;
        let mut image = None;
        for f in 0..frames {
            let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE.into())), time: Some(f as f64 / 60.0), ..Default::default() };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            image = gpu.frame(&ctx, input, |ui| setup.draw(ui), f + 1 == frames)?;
        }
        let path = dir.join(format!("{name}.png"));
        save_png(&path, &image.context("the frame gave no picture")?)?;
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_folders_are_shortened_in_the_middle() {
        let default = r"C:\Users\Enzo\AppData\Local\Programs\Orbiters - N4N0";
        assert_eq!(short_path(default, 64), default);
        let deep = r"C:\Users\Enzo\AppData\Local\Temp\claude\H--metaverse-ORBITERS-OrbitersDesktop\c2740cea\scratchpad\e2e\Orbiters - N4N0";
        assert_eq!(short_path(deep, 64), r"C:\Users\Enzo\…\e2e\Orbiters - N4N0");
        let long_name = format!(r"D:\{}", "x".repeat(100));
        let short = short_path(&long_name, 64);
        assert_eq!(short.chars().count(), 63);
        assert!(short.starts_with(r"D:\x") && short.contains('…'));
    }

    #[test]
    fn links_show_their_text() {
        assert_eq!(unlink("listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Nothing"), "listed in THIRD_PARTY_NOTICES.md. Nothing");
        assert_eq!(unlink("no link [here"), "no link [here");
        assert_eq!(unlink("[a](b) and [c](d)"), "a and c");
    }
}
