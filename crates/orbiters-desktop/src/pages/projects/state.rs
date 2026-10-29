use crate::pages::{Cx, Job};
use egui::{ColorImage, Id, Rect, TextureHandle};
use orbiters_core::account::Server;
use orbiters_projects::{
    avatars::Avatar,
    discovery::{self, Project, Scan},
    icons, network, setup, templates, vpm,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};

pub(super) enum ScanEvent {
    Update(Scan),
    Done(Result<Scan, String>),
}
#[derive(Clone, PartialEq)]
pub(super) enum Modal {
    Repos,
    New,
    Avatars,
    ChooseProject(String),
    Setup,
}
pub(super) struct Flow {
    pub project: Project,
    pub avatar: Avatar,
    pub request: Option<String>,
    pub unity_open: bool,
    pub result: Option<setup::ResultFile>,
    pub packages_only: bool,
    pub acknowledged: bool,
}
pub(super) enum Outcome {
    Changed(String),
    /// A project added to the list or created: its folder, and what to say.
    Added(PathBuf, String),
    Setup(Flow),
    Picked(Option<PathBuf>),
}
type ViewResult = Result<(vpm::View, Vec<templates::Template>, Vec<String>), String>;

/// A package's icon, loaded: a picture that fills its tile (solid nearly everywhere) or a mark to frame, and a mark dark
/// enough to need a light tile.
#[derive(Clone)]
pub(super) struct PackageIcon {
    pub tex: TextureHandle,
    pub fills: bool,
    pub dark: bool,
}
impl PackageIcon {
    pub fn load(ctx: &egui::Context, name: &str, image: &icons::Image) -> Self {
        let pixels = ColorImage::from_rgba_unmultiplied([image.size[0] as usize, image.size[1] as usize], &image.rgba);
        // Drawn at a third of their size or less: mipmaps keep them smooth.
        let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
        Self { tex: ctx.load_texture(format!("package-icon-{name}"), pixels, options), fills: image.fills(), dark: image.lightness() < 0.3 }
    }
}
/// Packages whose icons one batch looks for.
const ICON_BATCH: usize = 12;

pub struct State {
    pub(super) scan: Scan,
    pub(super) started: bool,
    pub(super) fixture: bool,
    pub(super) visit: u64,
    pub(super) scan_rx: Option<mpsc::Receiver<ScanEvent>>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) selected: Option<PathBuf>,
    pub(super) panel_serial: u64,
    /// The panel scrolls to its packages, or to Mixer's card, on its next frame (screenshots).
    pub(super) scroll_to_packages: bool,
    pub(super) scroll_to_mixer: bool,
    /// The docked panel's width as the person dragged it.
    pub(super) panel_width: Option<f32>,
    /// The sliding panel (narrow windows) is closing.
    pub(super) panel_closing: bool,
    /// Every project found on the computer listed, not the first few.
    pub(super) all_proposals: bool,
    /// Every package the repositories offer listed, not the first few.
    pub(super) all_packages: bool,
    /// The package an action changes (its row shows it working).
    pub(super) busy_package: Option<String>,
    pub(super) view: vpm::View,
    pub(super) view_job: Job<ViewResult>,
    pub(super) commits: Vec<String>,
    pub(super) templates: Vec<templates::Template>,
    pub(super) catalog: network::Catalog,
    pub(super) catalog_job: Job<network::Catalog>,
    pub(super) thumbs: HashMap<String, TextureHandle>,
    pub(super) thumb_ids: HashSet<String>,
    pub(super) thumbnail_environment: Option<crate::controller::vrchat_data::Scope>,
    pub(super) thumb_job: Job<Result<(HashMap<String, ColorImage>, Vec<String>), String>>,
    /// Package icons by package name: those looked for (once each), the batch looking, and the packages whose rows
    /// were drawn since the last look (only those are looked for).
    pub(super) icons: HashMap<String, PackageIcon>,
    pub(super) icons_asked: HashSet<String>,
    pub(super) icon_job: Job<Vec<(String, Option<icons::Image>)>>,
    pub(super) icons_wanted: Vec<String>,
    /// Creator Companion's template covers by file, and their loading.
    pub(super) banners: HashMap<PathBuf, TextureHandle>,
    pub(super) banner_job: Job<Vec<(PathBuf, Option<ColorImage>)>>,
    /// Mixer's card tells what Mixer adds.
    pub(super) mixer_info: bool,
    pub(super) modal: Option<Modal>,
    /// The dialog's serial (a new one for each dialog opened) and what it grew out of.
    pub(super) modal_serial: u64,
    pub(super) modal_origin: Rect,
    /// The colour of what the dialog grew out of.
    pub(super) modal_fill: egui::Color32,
    /// Bumped each time the panel shows another project (its cards crossfade).
    pub(super) switches: u64,
    /// Where the page's controls were drawn last frame (dialogs grow out of them; screenshots open them there).
    pub(super) anchors: HashMap<String, Rect>,
    /// The copy of the avatar picked in the project choice: its project and scene.
    pub(super) choice: Option<(PathBuf, String)>,
    /// What is being set up while the installation runs: the project's and the avatar's names, and N4N0 or My
    /// Avatar only.
    pub(super) pending: Option<(String, String, bool)>,
    /// A project added or created, opened once the scan lists it.
    pub(super) reveal: Option<PathBuf>,
    pub(super) flow: Option<Flow>,
    pub(super) result_job: Job<Result<Option<setup::ResultFile>, String>>,
    pub(super) ack_job: Job<Result<(), String>>,
    pub(super) result_at: Option<Instant>,
    pub(super) action: Job<Result<Outcome, String>>,
    pub(super) action_label: String,
    pub(super) error: Option<String>,
    pub(super) query: String,
    pub(super) package_query: String,
    pub(super) prerelease: bool,
    pub(super) repo_url: String,
    pub(super) repo_headers: String,
    pub(super) new_name: String,
    pub(super) new_location: String,
    pub(super) new_unity: String,
    pub(super) template_index: usize,
    pub(super) duplicate: bool,
    pub(super) duplicate_index: usize,
    pub(super) copy_cancel: Arc<AtomicBool>,
    pub(super) copy_progress: templates::SharedProgress,
    pub(super) preselected: Option<String>,
    /// Screenshots: a scan, the packages' reading or an action shown as running.
    pub(super) fixture_scanning: bool,
    pub(super) fixture_view_loading: bool,
    pub(super) fixture_action: Option<String>,
    /// Screenshots: avatar pictures, package icons and template covers that arrive later (they fade in).
    pub(super) fixture_later: HashMap<String, TextureHandle>,
    /// The selected project's 3D preview and the window it grows into.
    pub(super) preview: super::preview::Previews,
    pub(super) fixture_icons_later: HashMap<String, PackageIcon>,
    /// Screenshots (`projects-icons-live`): the stand-in packages' icons looked for for real (cache, then network).
    pub(super) fixture_live_icons: bool,
    pub(super) fixture_banners_later: HashMap<PathBuf, TextureHandle>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            scan: Scan::default(),
            started: false,
            fixture: false,
            visit: 0,
            scan_rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            selected: None,
            panel_serial: 0,
            scroll_to_packages: false,
            scroll_to_mixer: false,
            panel_width: None,
            panel_closing: false,
            all_proposals: false,
            all_packages: false,
            busy_package: None,
            view: vpm::View::default(),
            view_job: Job::default(),
            commits: vec![],
            templates: vec![],
            catalog: network::Catalog::default(),
            catalog_job: Job::default(),
            thumbs: HashMap::new(),
            thumb_ids: HashSet::new(),
            thumbnail_environment: None,
            thumb_job: Job::default(),
            icons: HashMap::new(),
            icons_asked: HashSet::new(),
            icon_job: Job::default(),
            icons_wanted: vec![],
            banners: HashMap::new(),
            banner_job: Job::default(),
            mixer_info: false,
            modal: None,
            modal_serial: 0,
            modal_origin: Rect::NOTHING,
            modal_fill: orbiters_ui::theme::surface_strong(),
            switches: 0,
            anchors: HashMap::new(),
            choice: None,
            pending: None,
            reveal: None,
            flow: None,
            result_job: Job::default(),
            ack_job: Job::default(),
            result_at: None,
            action: Job::default(),
            action_label: String::new(),
            error: None,
            query: String::new(),
            package_query: String::new(),
            prerelease: false,
            repo_url: String::new(),
            repo_headers: String::new(),
            new_name: String::new(),
            new_location: String::new(),
            new_unity: String::new(),
            template_index: 0,
            duplicate: false,
            duplicate_index: 0,
            copy_cancel: Arc::new(AtomicBool::new(false)),
            copy_progress: Arc::new(Mutex::default()),
            preselected: None,
            fixture_scanning: false,
            fixture_view_loading: false,
            fixture_action: None,
            fixture_later: HashMap::new(),
            preview: Default::default(),
            fixture_icons_later: HashMap::new(),
            fixture_live_icons: false,
            fixture_banners_later: HashMap::new(),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.copy_cancel.store(true, Ordering::Relaxed);
    }
}
/// Mixer's package, from the Orbiters repository.
pub(super) const MIXER: &str = "orbiters.mixer";
/// Mixer's presentation on the website.
pub(super) const MIXER_PAGE: &str = "https://orbiters.cc/mixer";

pub(super) fn server(cx: &Cx) -> Server {
    if cx.snap.account.development {
        Server::Development
    } else {
        Server::Production
    }
}
impl State {
    pub fn busy(&self) -> bool {
        self.scan_rx.is_some() || self.view_job.running() || self.thumb_job.running()
    }
    /// Looking for projects on the computer.
    pub(super) fn scanning(&self) -> bool {
        self.scan_rx.is_some() || self.fixture_scanning
    }
    /// Reading the selected project's packages and the repositories.
    pub(super) fn view_loading(&self) -> bool {
        self.view_job.running() || self.fixture_view_loading
    }
    /// An action runs (one at a time): what it says it does.
    pub(super) fn acting(&self) -> Option<&str> {
        if self.action.running() {
            Some(self.action_label.as_str())
        } else {
            self.fixture_action.as_deref()
        }
    }
    pub(super) fn running(&self) -> bool {
        self.acting().is_some()
    }
    /// The open dialog's id.
    pub(super) fn dialog_id(&self) -> Id {
        Id::unique(("unity-project-dialog", self.modal_serial))
    }
    /// Opens `modal`, growing out of `origin`; a dialog already open (and not closing) changes its contents instead.
    pub(super) fn open_modal(&mut self, ctx: &egui::Context, modal: Modal, origin: Rect) {
        let open = self.modal.is_some() && !orbiters_ui::dialog::is_closing(ctx, self.dialog_id());
        if !open {
            self.modal_serial += 1;
            self.modal_origin =
                if origin.is_finite() && origin.is_positive() { origin } else { Rect::from_center_size(ctx.content_rect().center(), egui::vec2(160.0, 48.0)) };
        }
        self.modal = Some(modal);
    }
    /// Starts closing the open dialog (it is dropped once its morph back ended).
    pub(super) fn close_modal(&self, ctx: &egui::Context) {
        if self.modal.is_some() {
            orbiters_ui::dialog::close(ctx, self.dialog_id());
        }
    }
    /// Where the control `name` was drawn last frame, if it was.
    pub(super) fn anchor(&self, name: &str) -> Rect {
        self.anchors.get(name).copied().unwrap_or(Rect::NOTHING)
    }
    pub(super) fn ensure(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            self.refresh(ctx, false);
            self.load_view(ctx, false);
        }
    }
    pub(super) fn refresh(&mut self, ctx: &egui::Context, force: bool) {
        if self.fixture {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let scan = discovery::cached();
            let _ = tx.send(ScanEvent::Update(scan.clone()));
            ctx.request_repaint();
            let result = discovery::discover(scan, force, &cancel, |s| {
                let _ = tx.send(ScanEvent::Update(s.clone()));
                ctx.request_repaint();
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(ScanEvent::Done(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn load_view(&mut self, ctx: &egui::Context, refresh: bool) {
        if self.fixture {
            return;
        }
        let path = self.selected.clone();
        self.view_job = Job::spawn(ctx, move || {
            let root = orbiters_projects::vcc_dir();
            let view = orbiters_projects::runtime().and_then(|rt| rt.block_on(vpm::view(&root, path.as_deref(), refresh))).map_err(|e| format!("{e:#}"))?;
            let commits = path
                .as_ref()
                .filter(|p| p.join(".git").exists())
                .and_then(|p| orbiters_projects::hidden_command("git").arg("-C").arg(p).args(["log", "-4", "--format=%h\x1f%s\x1f%an\x1f%ar"]).output().ok())
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(ToOwned::to_owned).collect())
                .unwrap_or_default();
            Ok((view, templates::list(&root), commits))
        });
    }
    pub(super) fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.selected.as_ref() == Some(&path) && !self.panel_closing {
            return;
        }
        // A panel already on screen changes its project (no slide); a closed one slides in again.
        if self.selected.is_none() || self.panel_closing {
            self.panel_serial += 1;
        }
        self.selected = Some(path);
        self.switches += 1;
        self.panel_closing = false;
        self.scroll_to_packages = false;
        if !self.fixture {
            self.view.packages.clear();
            self.commits.clear();
        }
        self.load_view(ctx, false);
    }
    pub(super) fn catalog(&mut self, ctx: &egui::Context, server: Server) {
        if !self.fixture {
            self.catalog_job = Job::spawn(ctx, move || network::catalog(server));
        }
    }
    pub(super) fn task(&mut self, ctx: &egui::Context, label: &str, f: impl FnOnce() -> anyhow::Result<Outcome> + Send + 'static) {
        if self.action.running() || self.fixture {
            return;
        }
        self.error = None;
        self.action_label = label.into();
        self.action = Job::spawn(ctx, move || f().map_err(|e| format!("{e:#}")));
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let environment = crate::controller::vrchat_data::Scope::of(cx.snap);
        if !self.fixture && self.thumbnail_environment.as_ref() != Some(&environment) {
            self.thumbnail_environment = Some(environment);
            self.thumb_ids.clear();
            self.thumbs.clear();
            self.thumb_job = Job::default();
        }
        if let Some(rx) = &self.scan_rx {
            let mut done = false;
            while let Ok(event) = rx.try_recv() {
                match event {
                    ScanEvent::Update(scan) => self.scan = scan,
                    ScanEvent::Done(result) => {
                        done = true;
                        match result {
                            Ok(scan) => self.scan = scan,
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
            }
            if done {
                self.scan_rx = None;
            }
        }
        if let Some(result) = self.view_job.take() {
            match result {
                Ok((view, templates, commits)) => {
                    if self.new_location.is_empty() {
                        self.new_location = view.default_path.display().to_string();
                    }
                    self.view = view;
                    self.templates = templates;
                    self.commits = commits;
                    self.load_banners(ctx);
                }
                Err(e) => self.error = Some(e),
            }
        }
        self.load_icons(ctx);
        if let Some(covers) = self.banner_job.take() {
            let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            for (file, image) in covers {
                if let Some(image) = image {
                    let tex = ctx.load_texture(format!("template-cover-{}", file.display()), image, options);
                    self.banners.insert(file, tex);
                }
            }
        }
        if let Some(c) = self.catalog_job.take() {
            self.catalog = c;
        }
        if let Some(result) = self.action.take() {
            self.action_label.clear();
            self.busy_package = None;
            match result {
                Ok(Outcome::Changed(message)) => {
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message);
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
                }
                Ok(Outcome::Added(path, message)) => {
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message);
                    self.reveal = Some(path);
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
                }
                Ok(Outcome::Setup(flow)) => {
                    self.flow = Some(flow);
                    self.pending = None;
                    self.open_modal(ctx, Modal::Setup, Rect::NOTHING);
                    self.result_at = None;
                    self.load_view(ctx, false);
                }
                Ok(Outcome::Picked(Some(p))) => self.new_location = p.display().to_string(),
                Ok(Outcome::Picked(None)) => {}
                Err(e) => {
                    self.error = Some(e);
                    self.pending = None;
                    self.load_view(ctx, false);
                }
            }
        }
        // A project just added or created: opened once the scan lists it.
        if let Some(path) = self.reveal.clone() {
            if self.scan.projects.iter().any(|p| p.path == path) {
                self.reveal = None;
                self.open(ctx, path);
            } else if !self.scanning() {
                self.reveal = None;
            }
        }
        if !self.fixture && self.started && !self.thumb_job.running() {
            let mut seen = HashSet::new();
            let ids: Vec<String> = self
                .scan
                .projects
                .iter()
                .flat_map(|p| p.avatars.iter().map(|a| a.id.clone()))
                .filter(|id| !self.thumb_ids.contains(id) && seen.insert(id.clone()))
                .take(8)
                .collect();
            if !ids.is_empty() {
                self.thumb_ids.extend(ids.iter().cloned());
                let backend = cx.ctl.clone();
                let scope = crate::controller::vrchat_data::Scope::of(cx.snap);
                self.thumb_job = Job::spawn(ctx, move || {
                    use crate::controller::vrchat_data::{Request, Response};
                    let mut images = HashMap::new();
                    let mut errors = Vec::new();
                    if let Response::Avatars { pictures, errors: failures } = backend.vrchat_data(scope, Request::Avatars { ids })? {
                        errors.extend(failures);
                        for (id, p) in pictures {
                            images.insert(id, ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.rgba));
                        }
                    }
                    Ok((images, errors))
                });
            }
        }
        if let Some(result) = self.thumb_job.take() {
            ctx.request_repaint(); // Publish this batch before requesting the next one.
            match result {
                Ok((images, errors)) => {
                    for (id, image) in images {
                        self.thumbs.insert(id.clone(), ctx.load_texture(format!("project-avatar-{id}"), image, egui::TextureOptions::LINEAR));
                    }
                    if !errors.is_empty() {
                        self.error = Some(format!("{} avatar picture(s) could not load: {}. Refresh to retry.", errors.len(), errors[0]));
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if !self.fixture && matches!(self.modal, Some(Modal::Setup)) && self.flow.as_ref().is_some_and(|f| f.request.is_some() && f.result.is_none()) {
            ctx.request_repaint_after(Duration::from_secs(1));
            if !self.result_job.running() && self.result_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                let f = self.flow.as_ref().unwrap();
                let path = f.project.path.clone();
                let id = f.request.clone().unwrap();
                self.result_at = Some(Instant::now());
                self.result_job = Job::spawn(ctx, move || setup::read_result(&path, &id).map_err(|e| e.to_string()));
            }
        }
        if let Some(result) = self.result_job.take() {
            match result {
                Ok(Some(result)) => {
                    if let Some(f) = &mut self.flow {
                        f.result = Some(result);
                    }
                }
                Ok(None) => {}
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(Err(e)) = self.ack_job.take() {
            self.error = Some(format!("Setup result was shown but could not be removed: {e}"));
        }
    }
    /// Looks for the icons of the packages drawn since the last look that were never looked for, a batch at a time.
    fn load_icons(&mut self, ctx: &egui::Context) {
        let wanted = std::mem::take(&mut self.icons_wanted);
        if (!self.fixture || self.fixture_live_icons) && !self.icon_job.running() {
            let mut batch: Vec<vpm::Package> = vec![];
            for name in wanted {
                if batch.len() < ICON_BATCH && !self.icons_asked.contains(&name) {
                    if let Some(p) = self.view.packages.iter().find(|p| p.name == name) {
                        self.icons_asked.insert(name);
                        batch.push(p.clone());
                    }
                }
            }
            if !batch.is_empty() {
                self.icon_job = Job::spawn(ctx, move || {
                    let sources: Vec<icons::Sources> = batch.iter().map(icons::sources).collect();
                    batch.into_iter().map(|p| p.name).zip(icons::resolve_all(&sources)).collect()
                });
            }
        }
        if let Some(found) = self.icon_job.take() {
            for (name, image) in found {
                if let Some(image) = image {
                    let icon = PackageIcon::load(ctx, &name, &image);
                    self.icons.insert(name, icon);
                }
            }
        }
    }
    /// Decodes the covers of the templates listed that are not loaded yet.
    fn load_banners(&mut self, ctx: &egui::Context) {
        let files: HashSet<PathBuf> = self.templates.iter().filter_map(|t| t.banner.clone()).filter(|f| !self.banners.contains_key(f)).collect();
        if files.is_empty() || self.banner_job.running() || self.fixture {
            return;
        }
        self.banner_job = Job::spawn(ctx, move || {
            files
                .into_iter()
                .map(|f| {
                    let image = image::open(&f).ok().map(|i| {
                        let i = i.to_rgba8();
                        ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], &i)
                    });
                    (f, image)
                })
                .collect()
        });
    }
    /// Adds Mixer to `project` from the Orbiters repository (added to the repositories first if needed).
    pub(super) fn add_mixer(&mut self, ctx: &egui::Context, project: &Project) {
        let path = project.path.clone();
        self.busy_package = Some(MIXER.into());
        self.task(ctx, "Adding Mixer…", move || {
            orbiters_projects::runtime()?.block_on(vpm::install_orbiters(&orbiters_projects::vcc_dir(), &path, &[MIXER]))?;
            Ok(Outcome::Changed("Mixer is in the project: open it in Unity to try it".into()))
        });
    }
    pub(super) fn acknowledge_result(&mut self, ctx: &egui::Context) {
        if self.fixture {
            return;
        }
        let Some(flow) = self.flow.as_mut() else { return };
        if flow.acknowledged || flow.result.is_none() {
            return;
        }
        let Some(id) = flow.request.clone() else { return };
        let path = flow.project.path.clone();
        flow.acknowledged = true;
        self.ack_job = Job::spawn(ctx, move || setup::take_result(&path, &id).map(|_| ()).map_err(|e| e.to_string()));
    }
    pub(super) fn install(&mut self, ctx: &egui::Context, project: Project, avatar: Avatar, nano: bool, server: Server, origin: Rect) {
        if nano && server.n4n0_gallery_asset_id().is_none() {
            self.error = Some("N4N0 has not been published on the production gallery yet. My Avatar can still be installed.".into());
            return;
        }
        self.open_modal(ctx, Modal::Setup, origin);
        self.flow = None;
        self.pending = Some((project.name.clone(), avatar.name.clone(), nano));
        self.task(ctx, if nano { "Installing My Avatar and preparing N4N0…" } else { "Installing My Avatar…" }, move || {
            orbiters_projects::runtime()?.block_on(vpm::install_my_avatar(&orbiters_projects::vcc_dir(), &project.path))?;
            let request = if nano {
                let r = setup::request(avatar.clone(), server.n4n0_gallery_asset_id().unwrap(), server.api(), env!("CARGO_PKG_VERSION"))?;
                setup::write(&project.path, &r)?;
                Some(r.id)
            } else {
                None
            };
            let unity_open = orbiters_projects::unity_open(&project.path);
            Ok(Outcome::Setup(Flow { project, avatar, request, unity_open, result: None, packages_only: !nano, acknowledged: false }))
        });
    }
    /// The registered projects' copies of the avatar `id`, with their projects.
    pub(super) fn copies(&self, id: &str) -> Vec<(Project, Avatar)> {
        self.scan.projects.iter().filter(|p| p.registered).flat_map(|p| p.avatars.iter().filter(|a| a.id == id).map(|a| (p.clone(), a.clone()))).collect()
    }
    pub(super) fn choose_avatar(&mut self, ctx: &egui::Context, id: String, server: Server) {
        self.preselected = Some(id.clone());
        let choices = self.copies(&id);
        if choices.len() == 1 {
            let (p, a) = choices.into_iter().next().unwrap();
            self.install(ctx, p, a, true, server, Rect::NOTHING);
        } else {
            self.choice = choices.first().map(|(p, a)| (p.path.clone(), a.scene.clone()));
            self.open_modal(ctx, Modal::ChooseProject(id), Rect::NOTHING);
        }
    }
    /// Deterministic offscreen samples: no filesystem scan, API requests, or project mutations.
    pub fn stage(&mut self, ctx: &egui::Context, name: &str) {
        self.fixture = true;
        self.started = true;
        self.scan_rx = None;
        let names = ["MasculineCanine", "Rexouium", "Nova", "Ultinova", "Kitsune", "Protogen"];
        let projects = [
            ("My avatars", 4, discovery::Kind::Avatars, 0.2),
            ("Weekend avatar", 1, discovery::Kind::Avatars, 26.0),
            ("Moonlight world", 0, discovery::Kind::Worlds, 50.0),
            ("Club outfits", 2, discovery::Kind::Avatars, 90.0),
            ("Avatar backup", 1, discovery::Kind::Avatars, 200.0),
            ("Old avatar project", 3, discovery::Kind::Avatars, 400.0),
            ("Test world", 0, discovery::Kind::Worlds, 700.0),
        ];
        let found = if name == "projects-proposals" { 14 } else { 4 };
        self.scan.projects = (0..3 + found)
            .map(|i| {
                let (label, count, kind, hours) = projects[i.min(projects.len() - 1)];
                let label = if i < projects.len() { label.to_string() } else { format!("Avatar project {}", i - 2) };
                let avatars = (0..count)
                    .map(|a| {
                        let k = (a + i) % names.len();
                        Avatar {
                            id: format!("avtr_00000000-0000-4000-8000-{:012}", k),
                            name: names[k].into(),
                            scene: format!("Assets/{}/{}.unity", names[k], if a == 0 { "Avatar" } else { "Scenes/Main" }),
                            path: names[k].into(),
                        }
                    })
                    .collect();
                Project {
                    path: PathBuf::from(format!("H:/Unity projects/{label}")),
                    name: label,
                    unity: if i == 5 { "2019.4.31f1".into() } else { "2022.3.22f1".into() },
                    kind,
                    modified: discovery::now() - (hours * 3600.0) as u64 - i as u64 * 7,
                    registered: i < 3,
                    missing: false,
                    avatars,
                }
            })
            .collect();
        // Stand-in portraits: a soft studio backdrop in each avatar's colour and its silhouette.
        let hues: [[f32; 3]; 6] = [[96., 116., 196.], [170., 98., 150.], [70., 160., 150.], [210., 140., 80.], [150., 110., 220.], [90., 170., 110.]];
        for (i, n) in names.iter().enumerate() {
            let (w, h) = (320usize, 320usize);
            let mut pixels = vec![0u8; w * h * 3];
            let c = hues[i];
            for y in 0..h {
                for x in 0..w {
                    let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                    let glow = (1.0 - ((fx - 0.5).powi(2) + (fy - 0.35).powi(2)).sqrt() * 1.6).clamp(0.0, 1.0);
                    let head = ((fx - 0.5) / 0.15).powi(2) + ((fy - 0.38) / 0.17).powi(2) < 1.0;
                    let ears =
                        (((fx - 0.37) / 0.05).powi(2) + ((fy - 0.2) / 0.1).powi(2) < 1.0) || (((fx - 0.63) / 0.05).powi(2) + ((fy - 0.2) / 0.1).powi(2) < 1.0);
                    let body = ((fx - 0.5) / 0.3).powi(2) + ((fy - 1.02) / 0.4).powi(2) < 1.0;
                    let col: [f32; 3] = if head || body || (ears && i % 2 == 0) {
                        let shade = 0.55 + 0.45 * (1.0 - fy);
                        [c[0] * shade + 40.0, c[1] * shade + 40.0, c[2] * shade + 40.0]
                    } else {
                        [14.0 + c[0] * 0.32 * glow, 14.0 + c[1] * 0.32 * glow, 18.0 + c[2] * 0.32 * glow]
                    };
                    let at = (y * w + x) * 3;
                    for k in 0..3 {
                        pixels[at + k] = col[k].clamp(0.0, 255.0) as u8;
                    }
                }
            }
            self.thumbs.insert(
                format!("avtr_00000000-0000-4000-8000-{:012}", i),
                ctx.load_texture(*n, ColorImage::from_rgb([w, h], &pixels), egui::TextureOptions::LINEAR),
            );
        }
        self.view.repos = vec![
            vpm::Repository {
                name: "VRChat official".into(),
                url: "https://packages.vrchat.com/official".into(),
                id: "official".into(),
                enabled: true,
                builtin: true,
            },
            vpm::Repository {
                name: "VRChat curated".into(),
                url: "https://packages.vrchat.com/curated".into(),
                id: "curated".into(),
                enabled: false,
                builtin: true,
            },
            vpm::Repository { name: "Orbiters".into(), url: vpm::ORBITERS_REPOSITORY.into(), id: "orbiters".into(), enabled: true, builtin: false },
        ];
        self.view.packages = [
            ("com.vrchat.avatars", "VRChat SDK – Avatars", Some("3.9.0"), vec!["3.9.0", "3.8.2", "3.8.1", "3.7.6"]),
            ("com.vrchat.base", "VRChat SDK – Base", Some("3.9.0"), vec!["3.9.0", "3.8.2", "3.8.1", "3.7.6"]),
            ("orbiters.toolkit", "Orbiters Toolkit", Some("0.5.0"), vec!["0.6.0-beta.2", "0.5.0", "0.4.5"]),
            ("orbiters.myavatar", "My Avatar", Some("0.9.14"), vec!["0.9.14", "0.9.12", "0.8.0"]),
            ("com.vrcfury.vrcfury", "VRCFury", Some("1.1434.0"), vec!["1.1434.0", "1.1300.0"]),
            ("orbiters.mixer", "Mixer", None, vec!["0.7.0", "0.6.2"]),
            ("orbiters.refit", "ReFit", None, vec!["0.7.3", "0.7.2"]),
            ("com.poiyomi.toon", "Poiyomi Toon Shader", None, vec!["9.2.67"]),
        ]
        .into_iter()
        .map(|(n, d, v, versions)| vpm::Package {
            name: n.into(),
            display_name: d.into(),
            installed: v.map(Into::into),
            versions: versions.into_iter().map(Into::into).collect(),
            ..Default::default()
        })
        .collect();
        // Creator Companion's covers on the templates (stand-ins; `-covers` reads VCC's own files when it is installed,
        // `-novcc` has none, as a computer without VCC).
        let template = |name: &str, id: &str, unity: &str, builtin: bool| templates::Template {
            name: name.into(),
            unity: unity.into(),
            builtin,
            id: id.into(),
            banner: Some(PathBuf::from(format!("covers/{id}.png"))),
            ..Default::default()
        };
        self.templates = vec![
            template("VRChat Avatar Project", "com.vrchat.template.avatar", "2022.3.22f1", true),
            template("VRChat World Project", "com.vrchat.template.world", "2022.3.22f1", true),
            template("Studio base", "com.studio.template.base", "", false),
        ];
        let app = orbiters_projects::vcc_app_dir().filter(|_| name == "projects-new-covers");
        for (i, t) in self.templates.iter_mut().enumerate() {
            if name.ends_with("-novcc") {
                t.banner = None;
                continue;
            }
            let real = app.as_ref().and_then(|app| templates::banner(app, &t.id)).and_then(|f| image::open(f).ok()).map(|i| {
                let i = i.to_rgba8();
                ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], &i)
            });
            let cover = real.unwrap_or_else(|| stand_in_cover(i));
            let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            self.banners.insert(t.banner.clone().unwrap(), ctx.load_texture(format!("fixture-cover-{i}"), cover, options));
        }
        for (package, look) in [
            ("com.vrchat.avatars", Look::Picture([30., 90., 200.])),
            ("com.vrchat.base", Look::Picture([30., 90., 200.])),
            ("orbiters.toolkit", Look::Wings),
            ("orbiters.myavatar", Look::Wings),
            (MIXER, Look::Wings),
            ("com.vrcfury.vrcfury", Look::Mark([24., 24., 28.])),
            ("com.poiyomi.toon", Look::Mark([255., 120., 190.])),
        ] {
            self.icons.insert(package.into(), stand_in_icon(ctx, package, look));
        }
        self.new_location = "H:/Unity projects".into();
        self.new_name = "My new avatar".into();
        self.catalog.known = vec![
            network::Listing {
                name: "VRCFury".into(),
                url: "https://vcc.vrcfury.com".into(),
                description: "Avatar tools and setup helpers".into(),
                ..Default::default()
            },
            network::Listing {
                name: "Poiyomi".into(),
                url: "https://poiyomi.github.io/vpm/index.json".into(),
                description: "Poiyomi Toon and Pro shaders".into(),
                ..Default::default()
            },
        ];
        self.catalog.public = vec![network::Listing {
            name: "Orbiters".into(),
            url: vpm::ORBITERS_REPOSITORY.into(),
            description: "My Avatar, Toolkit, Mixer and more".into(),
            ..Default::default()
        }];
        let first = self.scan.projects[0].clone();
        let flow = |unity_open: bool, result: Option<(setup::Status, &str)>, packages_only: bool| Flow {
            project: first.clone(),
            avatar: first.avatars[0].clone(),
            request: (!packages_only).then(|| "00000000-0000-4000-8000-000000000001".to_string()),
            unity_open,
            result: result.map(|(status, message)| setup::ResultFile {
                schema: 1,
                id: "00000000-0000-4000-8000-000000000001".into(),
                status,
                message: message.into(),
                finished_utc: "2026-10-29T10:00:00Z".into(),
                installed: vec![],
            }),
            packages_only,
            acknowledged: true,
        };
        self.commits = vec![
            "8e21f0a\x1fSave avatar outfit\x1fEnzo\x1f2 hours ago".into(),
            "620df02\x1fAdd expression menu\x1fEnzo\x1fyesterday".into(),
            "1b9c4d3\x1fTexture pass on the hoodie\x1fEnzo\x1f3 days ago".into(),
        ];
        match name {
            "projects-detail" | "projects-packages" | "projects-narrow-detail" | "projects-dropdown" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_packages = matches!(name, "projects-packages" | "projects-dropdown");
            }
            "projects-loading" => {
                self.scan.projects.clear();
                self.fixture_scanning = true;
            }
            "projects-scanning" => self.fixture_scanning = true,
            "projects-empty" => self.scan.projects.clear(),
            "projects-panel-loading" => {
                self.selected = Some(first.path.clone());
                self.view.packages.clear();
                self.commits.clear();
                self.fixture_view_loading = true;
            }
            "projects-error" => {
                self.error = Some("VCC's project list could not be read: settings.json is locked by another program. Close VCC and refresh.".into());
                self.scan.warnings = vec!["D: could not be searched: access denied".into()];
            }
            "projects-busy" => {
                self.selected = Some(first.path.clone());
                self.fixture_action = Some("Duplicating My avatars…".into());
                *self.copy_progress.lock().unwrap() = templates::Progress {
                    copied: 1840,
                    total: 4310,
                    file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                    phase: "Copying project".into(),
                };
            }
            "projects-search" => self.query = "avatar".into(),
            "projects-crossfade" => {
                self.fixture_later = std::mem::take(&mut self.thumbs);
                self.fixture_icons_later = std::mem::take(&mut self.icons);
                self.selected = Some(first.path.clone());
                self.scroll_to_packages = true;
            }
            "projects-new-covers" | "projects-new-novcc" => self.modal = Some(Modal::New),
            "projects-new-loading" => {
                self.modal = Some(Modal::New);
                self.fixture_banners_later = std::mem::take(&mut self.banners);
            }
            "projects-icons-live" => {
                // The packages' real listings and websites, their icons looked for as the page does.
                self.selected = Some(first.path.clone());
                self.scroll_to_packages = true;
                self.icons.clear();
                self.fixture_live_icons = true;
                for p in &mut self.view.packages {
                    let (repository, author, site) = match p.name.as_str() {
                        n if n.starts_with("com.vrchat.") => (None, Some("https://github.com/vrchat/packages"), None),
                        n if n.starts_with("orbiters.") => (Some(vpm::ORBITERS_REPOSITORY), Some("https://orbiters.cc"), None),
                        "com.vrcfury.vrcfury" => (None, None, Some("https://vrcfury.com")),
                        "com.poiyomi.toon" => (None, Some("https://github.com/poiyomi/PoiyomiToonShader"), None),
                        _ => (None, None, None),
                    };
                    p.repository = repository.map(Into::into);
                    p.author_url = author.map(Into::into);
                    p.site_url = site.map(Into::into);
                }
            }
            "projects-repos-novcc" => {
                self.modal = Some(Modal::Repos);
                self.view.repos.retain(|r| r.builtin);
                self.view.repos.iter_mut().for_each(|r| r.enabled = true);
            }
            "projects-mixer" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_mixer = true;
                self.mixer_info = true;
            }
            "projects-mixer-installed" => {
                self.selected = Some(first.path.clone());
                if let Some(p) = self.view.packages.iter_mut().find(|p| p.name == MIXER) {
                    p.installed = Some("0.7.0".into());
                }
            }
            "projects-mixer-adding" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_mixer = true;
                self.busy_package = Some(MIXER.into());
                self.fixture_action = Some("Adding Mixer…".into());
            }
            "projects-mixer-open-mid" | "projects-mixer-open" | "projects-mixer-fold-mid" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_mixer = true;
            }
            "projects-repos" => self.modal = Some(Modal::Repos),
            "projects-repos-loading" => {
                self.modal = Some(Modal::Repos);
                self.catalog = network::Catalog::default();
                self.catalog_job = Job::spawn(ctx, || {
                    std::thread::sleep(Duration::from_secs(3600));
                    network::Catalog::default()
                });
            }
            "projects-new" => self.modal = Some(Modal::New),
            "projects-new-custom" => {
                self.modal = Some(Modal::New);
                self.template_index = 2;
                self.new_name = "Studio/Base".into();
            }
            "projects-new-duplicate" => {
                self.modal = Some(Modal::New);
                self.duplicate = true;
                self.new_name = "My avatars copy".into();
            }
            "projects-new-copying" => {
                self.modal = Some(Modal::New);
                self.duplicate = true;
                self.new_name = "My avatars copy".into();
                self.fixture_action = Some("Creating project…".into());
                *self.copy_progress.lock().unwrap() = templates::Progress {
                    copied: 1840,
                    total: 4310,
                    file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                    phase: "Copying project".into(),
                };
            }
            "projects-avatars" => {
                self.modal = Some(Modal::Avatars);
                self.preselected = Some(first.avatars[0].id.clone());
            }
            "projects-avatars-loading" => {
                self.modal = Some(Modal::Avatars);
                self.scan.projects.clear();
                self.fixture_scanning = true;
            }
            "projects-avatars-empty" => {
                self.modal = Some(Modal::Avatars);
                self.scan.projects.clear();
            }
            "projects-choice" => {
                // An avatar saved in two projects.
                let id =
                    self.scan.projects.iter().flat_map(|p| p.avatars.iter()).map(|a| a.id.clone()).find(|id| self.copies(id).len() > 1).unwrap_or_default();
                self.choice = self.copies(&id).first().map(|(p, a)| (p.path.clone(), a.scene.clone()));
                self.modal = Some(Modal::ChooseProject(id));
            }
            "projects-installing" => {
                self.modal = Some(Modal::Setup);
                self.pending = Some((first.name.clone(), first.avatars[0].name.clone(), true));
                self.fixture_action = Some("Installing My Avatar and preparing N4N0…".into());
            }
            "projects-setup" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(true, None, false));
            }
            "projects-setup-closed" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(false, None, false));
            }
            "projects-setup-done" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(true, Some((setup::Status::Done, "N4N0 was added to MasculineCanine and the scene was saved.")), false));
            }
            "projects-setup-declined" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(true, Some((setup::Status::Declined, "You chose Not now in Unity. Nothing was changed.")), false));
            }
            "projects-setup-failed" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(
                    true,
                    Some((setup::Status::Failed, "The avatar's descriptor could not be found in Assets/MasculineCanine/Avatar.unity.")),
                    false,
                ));
            }
            "projects-installed" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(false, None, true));
            }
            _ => {}
        }
        if self.modal.is_some() {
            self.modal_serial = 1;
            self.modal_origin = Rect::from_center_size(ctx.content_rect().center() + egui::vec2(0.0, 120.0), egui::vec2(180.0, 44.0));
        }
    }
    /// Screenshots: what the person does at `frame` of the scenario `name` (a press on a control where it was drawn,
    /// the pointer over a card), for states reached through the interface and frames in the middle of their motion.
    pub fn stage_frame(&mut self, name: &str, frame: u32) -> Option<Poke> {
        if name == "projects-crossfade" && frame == 60 {
            self.thumbs = std::mem::take(&mut self.fixture_later);
            self.icons = std::mem::take(&mut self.fixture_icons_later);
        }
        if name == "projects-new-loading" && frame == 60 {
            self.banners = std::mem::take(&mut self.fixture_banners_later);
        }
        // Mixer added: its card folds away.
        if name == "projects-mixer-fold-mid" && frame == 60 {
            if let Some(p) = self.view.packages.iter_mut().find(|p| p.name == MIXER) {
                p.installed = Some("0.7.0".into());
            }
        }
        let at = |key: &str| self.anchors.get(key).map(|r| r.center());
        match (name, frame) {
            // The panel's edge dragged 160 points to the left, a frame at a time.
            ("projects-resize" | "projects-resize-mid", 50) => at("panel-grip").map(Poke::Hold),
            ("projects-resize" | "projects-resize-mid", 51..=58) => at("panel-grip").map(|p| Poke::Hover(p - egui::vec2(20.0 * (frame - 50) as f32, 0.0))),
            ("projects-new-mid" | "projects-new-open", 60) => at("new").map(Poke::Press),
            ("projects-repos-mid", 60) => at("repos").map(Poke::Press),
            ("projects-avatars-mid" | "projects-avatars-open", 60) => at("capture-card").map(Poke::Press),
            ("projects-sheet-mid" | "projects-sheet-open", 60) => at("card-1").map(Poke::Press),
            ("projects-switch-mid", 60) => at("card-1").map(Poke::Press),
            ("projects-press", 80) => at("card-1").map(Poke::Hold),
            ("projects-hover", 40..) => at("card-0").map(Poke::Hover),
            ("projects-dropdown", 60) => at("version-0").map(Poke::Press),
            ("projects-menu", 60) => at("project-menu").map(Poke::Press),
            ("projects-setup-from-panel", 60) => at("add-n4n0-0").map(Poke::Press),
            ("projects-mixer-open-mid" | "projects-mixer-open", 60) => at("mixer-info").map(Poke::Press),
            _ => None,
        }
    }
}
/// How a stand-in package icon looks (screenshots).
#[derive(Clone, Copy)]
enum Look {
    /// A picture filling its tile: a gradient in this colour and a white speech bubble.
    Picture([f32; 3]),
    /// Orbiters' wings on a dark square, as orbiters.cc's icon.
    Wings,
    /// A diamond in this colour on nothing (a mark to frame).
    Mark([f32; 3]),
}

fn stand_in_icon(ctx: &egui::Context, name: &str, look: Look) -> PackageIcon {
    let n = 96usize;
    let wings = orbiters_ui::n4n0_face::rasterize(64, 57, egui::Color32::WHITE);
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            let c: [f32; 4] = match look {
                Look::Picture(c) => {
                    let bubble = (fx - 0.5).abs() < 0.3 && (fy - 0.47).abs() < 0.16 || ((fx - 0.4).abs() < 0.06 && fy > 0.6 && fy < 0.72);
                    if bubble {
                        [255.0, 255.0, 255.0, 255.0]
                    } else {
                        let k = 0.75 + 0.5 * (1.0 - fy);
                        [c[0] * k, c[1] * k, c[2] * k, 255.0]
                    }
                }
                Look::Wings => {
                    let (wx, wy) = (x as i64 - 16, y as i64 - 20);
                    let a = if (0..64).contains(&wx) && (0..57).contains(&wy) { wings.pixels[wy as usize * 64 + wx as usize].a() as f32 / 255.0 } else { 0.0 };
                    let bg = [18.0, 19.0, 24.0];
                    [bg[0] + (230.0 - bg[0]) * a, bg[1] + (230.0 - bg[1]) * a, bg[2] + (235.0 - bg[2]) * a, 255.0]
                }
                Look::Mark(c) => {
                    let d = (fx - 0.5).abs() + (fy - 0.5).abs();
                    let a = ((0.36 - d) * 60.0).clamp(0.0, 1.0) * if d < 0.16 { 0.0 } else { 1.0 };
                    [c[0], c[1], c[2], 255.0 * a]
                }
            };
            for k in 0..4 {
                rgba[(y * n + x) * 4 + k] = c[k].clamp(0.0, 255.0) as u8;
            }
        }
    }
    PackageIcon::load(ctx, name, &icons::Image { size: [n as u32, n as u32], rgba })
}

/// A stand-in template cover (screenshots): a 400 × 50 strip in the template's colour with soft lights.
fn stand_in_cover(i: usize) -> ColorImage {
    let (w, h) = (400usize, 50usize);
    let base = [[40., 70., 170.], [110., 60., 170.], [30., 120., 130.]][i % 3];
    let mut pixels = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let glow = (1.0 - ((fx - 0.75) * 3.0).powi(2) - ((fy - 0.4) * 1.6).powi(2)).max(0.0);
            let k = 0.45 + 0.35 * fx + 0.6 * glow;
            for c in 0..3 {
                pixels[(y * w + x) * 3 + c] = (base[c] * k).clamp(0.0, 255.0) as u8;
            }
        }
    }
    ColorImage::from_rgb([w, h], &pixels)
}

/// What a screenshot scenario does with the pointer on one frame.
#[derive(Clone, Copy, Debug)]
pub enum Poke {
    /// Pressed there, released on the next frame.
    Press(egui::Pos2),
    /// Pressed there and held.
    Hold(egui::Pos2),
    /// Over it.
    Hover(egui::Pos2),
}
