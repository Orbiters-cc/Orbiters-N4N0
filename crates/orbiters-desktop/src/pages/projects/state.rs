use crate::pages::{Cx, Job};
use egui::{ColorImage, TextureHandle};
use orbiters_core::account::Server;
use orbiters_projects::{
    avatars::Avatar,
    discovery::{self, Project, Scan},
    network, setup, templates, vpm,
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
#[derive(Clone)]
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
    Setup(Flow),
    Picked(Option<PathBuf>),
}
type ViewResult = Result<(vpm::View, Vec<templates::Template>, Vec<String>), String>;

pub struct State {
    pub(super) scan: Scan,
    pub(super) started: bool,
    pub(super) fixture: bool,
    pub(super) visit: u64,
    pub(super) scan_rx: Option<mpsc::Receiver<ScanEvent>>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) selected: Option<PathBuf>,
    pub(super) panel_serial: u64,
    pub(super) packages_only: bool,
    pub(super) view: vpm::View,
    pub(super) view_job: Job<ViewResult>,
    pub(super) commits: Vec<String>,
    pub(super) templates: Vec<templates::Template>,
    pub(super) catalog: network::Catalog,
    pub(super) catalog_job: Job<network::Catalog>,
    pub(super) thumbs: HashMap<String, TextureHandle>,
    pub(super) thumb_ids: HashSet<String>,
    pub(super) thumbnail_environment: Option<(bool, Option<String>)>,
    pub(super) thumb_job: Job<HashMap<String, ColorImage>>,
    pub(super) modal: Option<Modal>,
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
            packages_only: false,
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
            modal: None,
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
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.copy_cancel.store(true, Ordering::Relaxed);
    }
}
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
        self.selected = Some(path);
        self.panel_serial += 1;
        self.packages_only = false;
        self.view.packages.clear();
        self.commits.clear();
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
        let environment = (cx.snap.account.development, cx.snap.account.state.username().map(ToOwned::to_owned));
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
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(c) = self.catalog_job.take() {
            self.catalog = c;
        }
        if let Some(result) = self.action.take() {
            self.action_label.clear();
            match result {
                Ok(Outcome::Changed(message)) => {
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message);
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
                }
                Ok(Outcome::Setup(flow)) => {
                    self.flow = Some(flow);
                    self.modal = Some(Modal::Setup);
                    self.result_at = None;
                    self.load_view(ctx, false);
                }
                Ok(Outcome::Picked(Some(p))) => self.new_location = p.display().to_string(),
                Ok(Outcome::Picked(None)) => {}
                Err(e) => {
                    self.error = Some(e);
                    self.load_view(ctx, false);
                }
            }
        }
        if !self.fixture && self.started && !self.thumb_job.running() {
            let ids: HashSet<String> = self.scan.projects.iter().flat_map(|p| p.avatars.iter().map(|a| a.id.clone())).collect();
            if ids != self.thumb_ids {
                self.thumb_ids = ids.clone();
                let server = server(cx);
                self.thumb_job = Job::spawn(ctx, move || {
                    let mut paths = HashMap::new();
                    for id in &ids {
                        if let Some(p) = orbiters_projects::avatars::local_thumbnail(id) {
                            paths.insert(id.clone(), p);
                        }
                    }
                    if let Ok(remote) = network::thumbnails(server, ids.into_iter().collect()) {
                        paths.extend(remote.into_iter().map(|(id, t)| (id, t.path)));
                    }
                    paths
                        .into_iter()
                        .filter_map(|(id, p)| {
                            let rgb = orbiters_orb::picture::rgb_any(&std::fs::read(p).ok()?).ok()?;
                            let image = orbiters_orb::picture::letterbox(&rgb, 480, 360, [24, 24, 27]);
                            Some((id, ColorImage::from_rgb([480, 360], &image.pixels)))
                        })
                        .collect()
                });
            }
        }
        if let Some(images) = self.thumb_job.take() {
            for (id, image) in images {
                self.thumbs.insert(id.clone(), ctx.load_texture(format!("project-avatar-{id}"), image, egui::TextureOptions::LINEAR));
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
    pub(super) fn install(&mut self, ctx: &egui::Context, project: Project, avatar: Avatar, nano: bool, server: Server) {
        if nano && server.n4n0_gallery_asset_id().is_none() {
            self.error = Some("N4N0 has not been published on the production gallery yet. My Avatar can still be installed.".into());
            return;
        }
        self.modal = Some(Modal::Setup);
        self.flow = None;
        self.task(ctx, if nano { "Installing My Avatar and preparing N4N0…" } else { "Installing My Avatar…" }, move || {
            orbiters_projects::runtime()?.block_on(vpm::install_my_avatar(&orbiters_projects::vcc_dir(), &project.path))?;
            let request = if nano {
                let r = setup::request(avatar.clone(), server.n4n0_gallery_asset_id().unwrap(), env!("CARGO_PKG_VERSION"))?;
                setup::write(&project.path, &r)?;
                Some(r.id)
            } else {
                None
            };
            let unity_open = orbiters_projects::unity_open(&project.path);
            Ok(Outcome::Setup(Flow { project, avatar, request, unity_open, result: None, packages_only: !nano, acknowledged: false }))
        });
    }
    pub(super) fn choose_avatar(&mut self, ctx: &egui::Context, id: String, server: Server) {
        self.preselected = Some(id.clone());
        let choices: Vec<_> =
            self.scan.projects.iter().filter(|p| p.registered).flat_map(|p| p.avatars.iter().filter(|a| a.id == id).map(|a| (p.clone(), a.clone()))).collect();
        if choices.len() == 1 {
            let (p, a) = choices.into_iter().next().unwrap();
            self.install(ctx, p, a, true, server);
        } else {
            self.modal = Some(Modal::ChooseProject(id));
        }
    }
    /// Deterministic offscreen samples: no filesystem scan, API requests, or project mutations.
    pub fn stage(&mut self, ctx: &egui::Context, name: &str) {
        self.fixture = true;
        self.started = true;
        self.scan_rx = None;
        let names = ["MasculineCanine", "Rexouium", "Nova"];
        self.scan.projects = (0..5)
            .map(|i| {
                let avatars = if i == 2 {
                    vec![]
                } else {
                    (0..if i == 0 { 3 } else { 1 })
                        .map(|a| Avatar {
                            id: format!("avtr_00000000-0000-4000-8000-{:012}", a),
                            name: names[a].into(),
                            scene: format!("Assets/{}/Avatar.unity", names[a]),
                            path: names[a].into(),
                        })
                        .collect()
                };
                Project {
                    path: PathBuf::from(format!(
                        "H:/Unity projects/{}",
                        ["My avatars", "Weekend avatar", "Moonlight world", "Avatar backup", "Old avatar project"][i]
                    )),
                    name: ["My avatars", "Weekend avatar", "Moonlight world", "Avatar backup", "Old avatar project"][i].into(),
                    unity: "2022.3.22f1".into(),
                    kind: if i == 2 { discovery::Kind::Worlds } else { discovery::Kind::Avatars },
                    modified: discovery::now() - i as u64 * 86400,
                    registered: i < 3,
                    missing: false,
                    avatars,
                }
            })
            .collect();
        // Real, checked-in artwork when available; the generated neutral silhouettes cover a missing sample asset.
        for (i, n) in names.iter().enumerate() {
            let mut pixels = vec![0; 320 * 240 * 3];
            for y in 0..240 {
                for x in 0..320 {
                    let d = ((x as f32 - 160.).powi(2) + (y as f32 - 104.).powi(2)).sqrt();
                    let body = ((x as f32 - 160.) / 75.).powi(2) + ((y as f32 - 238.) / 90.).powi(2) < 1.;
                    let col =
                        if d < 47. || body { [[99, 108, 133], [122, 104, 125], [98, 126, 128]][i] } else { [28 + (x / 32) as u8, 29 + (y / 30) as u8, 34] };
                    let at = (y * 320 + x) * 3;
                    pixels[at..at + 3].copy_from_slice(&col);
                }
            }
            self.thumbs.insert(
                format!("avtr_00000000-0000-4000-8000-{:012}", i),
                ctx.load_texture(*n, ColorImage::from_rgb([320, 240], &pixels), egui::TextureOptions::LINEAR),
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
            vpm::Repository { name: "Orbiters".into(), url: vpm::ORBITERS_REPOSITORY.into(), id: "orbiters".into(), enabled: true, builtin: false },
        ];
        self.view.packages = [
            ("com.vrchat.avatars", "VRChat SDK – Avatars", "3.9.0"),
            ("com.vrchat.base", "VRChat SDK – Base", "3.9.0"),
            ("orbiters.toolkit", "Orbiters Toolkit", "0.5.0"),
            ("orbiters.myavatar", "My Avatar", "0.8.0"),
        ]
        .into_iter()
        .map(|(n, d, v)| vpm::Package { name: n.into(), display_name: d.into(), installed: Some(v.into()), versions: vec![v.into(), "0.4.0".into()] })
        .chain(std::iter::once(vpm::Package { name: "orbiters.mixer".into(), display_name: "Mixer".into(), installed: None, versions: vec!["0.7.0".into()] }))
        .collect();
        self.templates = vec![
            templates::Template { name: "VRChat Avatar Project".into(), unity: "2022.3.22f1".into(), builtin: true, ..Default::default() },
            templates::Template { name: "VRChat World Project".into(), unity: "2022.3.22f1".into(), builtin: true, ..Default::default() },
        ];
        self.new_location = "H:/Unity projects".into();
        self.new_name = "My new avatar".into();
        self.catalog.known =
            vec![network::Listing { name: "VRCFury".into(), url: "https://vcc.vrcfury.com".into(), description: "Avatar tools and setup helpers".into() }];
        self.catalog.public =
            vec![network::Listing { name: "Orbiters".into(), url: vpm::ORBITERS_REPOSITORY.into(), description: "My Avatar, Toolkit, Mixer and more".into() }];
        match name {
            "projects-detail" | "projects-packages" => {
                self.selected = Some(self.scan.projects[0].path.clone());
                self.packages_only = name.ends_with("packages");
                self.commits =
                    vec!["8e21f0a\x1fSave avatar outfit\x1fEnzo\x1f2 hours ago".into(), "620df02\x1fAdd expression menu\x1fEnzo\x1fYesterday".into()];
            }
            "projects-repos" => self.modal = Some(Modal::Repos),
            "projects-new" => self.modal = Some(Modal::New),
            "projects-avatars" => {
                self.modal = Some(Modal::Avatars);
                self.preselected = Some(self.scan.projects[0].avatars[0].id.clone());
            }
            "projects-choice" => self.modal = Some(Modal::ChooseProject(self.scan.projects[0].avatars[0].id.clone())),
            "projects-setup" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(Flow {
                    project: self.scan.projects[0].clone(),
                    avatar: self.scan.projects[0].avatars[0].clone(),
                    request: Some(uuid::Uuid::new_v4().to_string()),
                    unity_open: true,
                    result: None,
                    packages_only: false,
                    acknowledged: false,
                });
            }
            _ => {}
        }
    }
}
