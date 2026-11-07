use orbiters_i18n::{mark, tr, tr_n};
use super::thumb_cache;
use crate::pages::{Cx, Job};
use egui::{ColorImage, Id, Rect, TextureHandle};
use orbiters_core::account::Server;
use orbiters_projects::{
    avatars::Avatar,
    blender,
    discovery::{self, Project, Scan},
    editor, git, hidden, icons, network, setup, size, templates, updates, vpm,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};

pub(super) enum ScanEvent {
    /// The projects hidden from the lists, read before the cached scan.
    Hidden(BTreeSet<String>),
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
    /// A copy of the project in this folder.
    Duplicate(PathBuf),
    /// A package change that conflicted, and the plan solving it.
    Conflict,
}
/// The Duplicate dialog's choices: the copy's name and where it goes, Library taken along, and what was found of that
/// place (on a worker, when they change).
#[derive(Default)]
pub(super) struct Dup {
    pub measured: Option<(bool, Result<u64, String>)>,
    pub measure: Job<(bool, Result<u64, String>)>,
    pub name: String,
    pub location: String,
    pub library: bool,
    /// The (location, name) last checked: whether that folder exists already, and the room on its disk.
    pub checked: Option<(String, String, bool, Option<u64>)>,
    pub check: Job<(String, String, bool, Option<u64>)>,
    /// The folder dialog of its location.
    pub pick: Job<Option<PathBuf>>,
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
    /// Done, with only this to say (a program opened).
    Said(String),
    /// A package change refused for a conflict: the project and the plan making it coherent.
    Conflict(PathBuf, orbiters_projects::coherent::Plan),
}
type ViewResult = Result<(vpm::View, Vec<templates::Template>, Option<git::Repo>), String>;

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

/// A batch of pictures asked for: the different ones with their sums, the ids whose picture was the same, the errors.
pub(super) type ThumbBatch = (HashMap<String, (ColorImage, u64)>, Vec<String>, Vec<String>, HashMap<String, u64>);

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
    /// The panel scrolls to its last card, the Git repository's (screenshots).
    pub(super) scroll_to_commits: bool,
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
    /// The selected project's Git repository (its branch, changes, last commits, remote page), when it is one.
    pub(super) git: Option<git::Repo>,
    /// Commits opened to their whole message.
    pub(super) commit_open: HashSet<String>,
    pub(super) templates: Vec<templates::Template>,
    pub(super) catalog: network::Catalog,
    pub(super) catalog_job: Job<network::Catalog>,
    /// The catalog's repositories as the dialog lists them, and what the search finds among them: kept, as it is asked for
    /// every frame ([`Self::set_catalog`]).
    pub(super) suggestions: orbiters_projects::suggestions::Suggestions,
    /// What is typed in the dialog's search.
    pub(super) repo_query: String,
    /// The suggested repository being added, for its card to say so.
    pub(super) repo_adding: Option<String>,
    pub(super) thumbs: HashMap<String, TextureHandle>,
    /// The sum ([`thumb_cache::sum`]) of the picture each texture shows: a picture asked for again changes nothing
    /// when it is the same.
    pub(super) thumb_sums: HashMap<String, u64>,
    /// A picture replaced by a different one: the one before, drawn under the new one while that fades in (the
    /// serial names the fade).
    pub(super) thumb_before: HashMap<String, (TextureHandle, u64)>,
    pub(super) thumb_swaps: u64,
    /// The pictures kept on this computer by the last window, being read ([`thumb_cache::preload`]).
    pub(super) thumb_preload: Option<mpsc::Receiver<Vec<thumb_cache::Cached>>>,
    /// Pictures asked for (or not to ask for) in this window.
    pub(super) thumb_ids: HashSet<String>,
    pub(super) thumb_versions: Arc<Mutex<HashMap<String, u64>>>,
    thumb_preload_waited: bool,
    /// Pictures checked less than [`thumb_cache::FRESH`] ago: never asked for again meanwhile, whatever the account,
    /// VRChat session or update (only Refresh asks for them).
    pub(super) thumb_fresh: HashSet<String>,
    /// Pictures shown from this computer's copy: drawn at once, without the fade of a picture arriving.
    pub(super) thumb_instant: HashSet<String>,
    pub(super) thumbnail_environment: Option<crate::controller::vrchat_data::Scope>,
    pub(super) thumb_job: Job<Result<ThumbBatch, String>>,
    /// Package icons by package name: those looked for (once each), the batch looking, and the packages whose rows
    /// were drawn since the last look (only those are looked for).
    pub(super) icons: HashMap<String, PackageIcon>,
    pub(super) icons_asked: HashSet<String>,
    pub(super) icon_job: Job<Vec<(String, Option<icons::Image>)>>,
    pub(super) icons_wanted: Vec<String>,
    /// Repositories' icons by listing URL, as packages' (their website's icon, or the logo Orbiters keeps).
    pub(super) repo_icons: HashMap<String, PackageIcon>,
    pub(super) repo_icons_asked: HashSet<String>,
    pub(super) repo_icon_job: Job<Vec<(String, Option<icons::Image>)>>,
    pub(super) repo_icons_wanted: Vec<String>,
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
    /// The running action adds repositories (the missing packages card shows its Retry busy meanwhile).
    pub(super) adding_repositories: bool,
    pub(super) storage_job: Job<(PathBuf, orbiters_projects::storage::Health)>,
    pub(super) storage_health: Option<(PathBuf, orbiters_projects::storage::Health)>,
    pub(super) storage_checked: Option<Instant>,
    pub(super) storage: super::storage_card::Storage,
    /// Screenshots: a real run's Repair storage was pressed.
    pub(super) stage_repair_started: bool,
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
    pub(super) fixture_repo_icons_later: HashMap<String, PackageIcon>,
    /// Screenshots (`projects-revisit`): the pictures asked for again after the page's second visit, the same but one.
    pub(super) fixture_arrivals: Vec<(String, TextureHandle, u64)>,
    /// Projects hidden from the lists ([`hidden`], by [`discovery::key`] of their folder), how many changes were made
    /// to it here (each save numbered; a list read later than one never replaces it), and whether the lists show them
    /// anyway, dimmed and marked.
    pub(super) hidden: BTreeSet<String>,
    pub(super) hidden_changes: u64,
    pub(super) show_hidden: bool,
    /// Projects' sizes on disk as last measured ([`size`]), the projects looked up since the page opened, the lookup
    /// and the walk running (one project at a time) and its cancel.
    pub(super) sizes: HashMap<PathBuf, size::Size>,
    pub(super) sizes_asked: HashSet<PathBuf>,
    pub(super) size_lookup: Job<(PathBuf, size::Lookup)>,
    pub(super) size_walk: Job<(PathBuf, Result<size::Size, String>)>,
    pub(super) size_waiting: Vec<PathBuf>,
    pub(super) size_cancel: Arc<AtomicBool>,
    /// The Duplicate dialog's choices.
    pub(super) dup: Dup,
    /// The selected project's Blender files ([`blender`]), read when it is opened, and for which project.
    pub(super) links: Vec<blender::Link>,
    pub(super) links_of: Option<PathBuf>,
    pub(super) links_job: Job<(PathBuf, Vec<blender::Link>)>,
    /// World projects' worlds as this computer has them (the captures recorded and the pictures taken there), read
    /// once per visit and library change, for the Worlds card.
    pub(super) here: HashMap<String, super::world_card::Here>,
    pub(super) here_key: Option<(u64, PathBuf, u64)>,
    pub(super) here_job: Job<HashMap<String, super::world_card::Here>>,
    /// Pictures picked to add to a world (the file dialog runs on a worker): the world's ID and name, and the files.
    pub(super) pick_job: Job<Option<(String, String, Vec<PathBuf>)>>,
    /// Uploads handed to the tray service, by world: when, and what (shown as starting until the service lists them).
    pub(super) handed: HashMap<String, (f64, crate::state::UploadKind)>,
    /// The visit Orbiters was last asked what this member added to its Map explorer.
    pub(super) maps_visit: Option<u64>,
    /// Screenshots: what this member added on Orbiters, and the uploads, instead of the controller's.
    pub(super) fixture_maps: Option<crate::state::MapsMine>,
    pub(super) fixture_uploads: Vec<crate::state::UploadStatus>,
    /// Screenshots: the cards kept scrolled down by this much (under the header's blur).
    pub(super) scroll_gallery: Option<f32>,
    /// The member's avatars and worlds whose pictures can be changed on VRChat from their thumbnails.
    pub(super) vrchat_pictures: super::vrchat_picture::Pictures,
    /// The selected project's package updates (for releases and with pre-releases) and the check running.
    pub(super) updates: Option<(PathBuf, updates::Check)>,
    pub(super) updates_job: Job<(PathBuf, Result<updates::Check, String>)>,
    /// The repositories last downloaded again for the update check (once a window, then every half hour).
    pub(super) repos_checked: Option<Instant>,
    /// The view being read is that periodic look: its failure is not shown (the cached view stays).
    pub(super) view_quiet: bool,
    /// Update all's plan opened, waiting for its confirmation.
    pub(super) plan_open: bool,
    /// The package change last started: retried once the missing packages' repositories are added.
    pub(super) last_change: Option<(PathBuf, vpm::Change)>,
    /// Packages a project needs that no repository lists: the error card with "Add their repositories".
    pub(super) missing: Option<super::missing_card::Card>,
    /// Each project's Unity editor (by `discovery::key`), and since when Unity's save question was answered.
    pub(super) sessions: HashMap<String, (editor::Session, Option<Instant>)>,
    pub(super) editors_job: Job<HashMap<String, Option<(u32, bool)>>>,
    pub(super) editors_at: Option<Instant>,
    /// Show, Save & Exit or Kill running (not the page's one action: Unity may be asked while packages change).
    pub(super) editor_action: Job<Result<(), String>>,
    /// The project whose Kill waits for its confirmation.
    pub(super) kill_confirm: Option<PathBuf>,
    /// The conflict dialog's plan.
    pub(super) conflict: Option<super::conflict::View>,
    /// The Orbiters server the window uses (where missing packages are looked up), from the last frame.
    pub(super) server_now: Option<Server>,
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
            scroll_to_commits: false,
            panel_width: None,
            panel_closing: false,
            all_proposals: false,
            all_packages: false,
            busy_package: None,
            view: vpm::View::default(),
            view_job: Job::default(),
            git: None,
            commit_open: HashSet::new(),
            templates: vec![],
            catalog: network::Catalog::default(),
            catalog_job: Job::default(),
            suggestions: Default::default(),
            repo_query: String::new(),
            repo_adding: None,
            thumbs: HashMap::new(),
            thumb_sums: HashMap::new(),
            thumb_before: HashMap::new(),
            thumb_swaps: 0,
            // Read at the window's start, so that the page's first visit shows them; never in unit tests.
            thumb_preload: (!cfg!(test)).then(thumb_cache::preload),
            thumb_ids: HashSet::new(),
            thumb_versions: Arc::default(),
            thumb_preload_waited: false,
            thumb_fresh: HashSet::new(),
            thumb_instant: HashSet::new(),
            thumbnail_environment: None,
            thumb_job: Job::default(),
            icons: HashMap::new(),
            icons_asked: HashSet::new(),
            icon_job: Job::default(),
            icons_wanted: vec![],
            repo_icons: HashMap::new(),
            repo_icons_asked: HashSet::new(),
            repo_icon_job: Job::default(),
            repo_icons_wanted: vec![],
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
            adding_repositories: false,
            storage_job: Job::default(),
            storage_health: None,
            storage_checked: None,
            storage: Default::default(),
            stage_repair_started: false,
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
            fixture_repo_icons_later: HashMap::new(),
            fixture_arrivals: vec![],
            hidden: BTreeSet::new(),
            hidden_changes: 0,
            show_hidden: false,
            sizes: HashMap::new(),
            sizes_asked: HashSet::new(),
            size_lookup: Job::default(),
            size_walk: Job::default(),
            size_waiting: vec![],
            size_cancel: Arc::new(AtomicBool::new(false)),
            dup: Dup::default(),
            links: vec![],
            links_of: None,
            links_job: Job::default(),
            here: HashMap::new(),
            here_key: None,
            here_job: Job::default(),
            pick_job: Job::default(),
            handed: HashMap::new(),
            maps_visit: None,
            fixture_maps: None,
            fixture_uploads: vec![],
            scroll_gallery: None,
            vrchat_pictures: Default::default(),
            updates: None,
            updates_job: Job::default(),
            repos_checked: None,
            view_quiet: false,
            plan_open: false,
            last_change: None,
            missing: None,
            sessions: HashMap::new(),
            editors_job: Job::default(),
            editors_at: None,
            editor_action: Job::default(),
            kill_confirm: None,
            conflict: None,
            server_now: None,
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.copy_cancel.store(true, Ordering::Relaxed);
        self.size_cancel.store(true, Ordering::Relaxed);
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
        if !open && modal == Modal::Repos {
            self.repo_query.clear();
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
        // The window's first look shows the projects the last one knew at once (a small file), so their covers are
        // there on the first frame; the scan then goes on from them.
        if self.scan.projects.is_empty() {
            self.scan = discovery::cached();
        }
        // A revisit must not replace live discoveries with an older (or temporarily unreadable) disk snapshot.
        let retained = (!self.scan.projects.is_empty()).then(|| self.scan.clone());
        std::thread::spawn(move || {
            let _ = tx.send(ScanEvent::Hidden(hidden::load()));
            let scan = retained.unwrap_or_else(discovery::cached);
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
        self.view_quiet = false;
        let path = self.selected.clone();
        self.view_job = Job::spawn(ctx, move || {
            let root = orbiters_projects::vcc_dir();
            let view = orbiters_projects::runtime().and_then(|rt| rt.block_on(vpm::view(&root, path.as_deref(), refresh))).map_err(|e| format!("{e:#}"))?;
            let repo = path.as_deref().and_then(git::read);
            Ok((view, templates::list(&root), repo))
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
            self.git = None;
            self.links.clear();
            self.links_of = None;
        }
        self.load_view(ctx, false);
        self.load_links(ctx);
    }
    /// Checks the selected project's packages for updates (after each look at its packages).
    fn check_updates(&mut self, ctx: &egui::Context) {
        if self.fixture {
            return;
        }
        let Some(path) = self.selected.clone() else { return };
        self.updates_job = Job::spawn(ctx, move || {
            let r = orbiters_projects::runtime()
                .and_then(|rt| rt.block_on(updates::check(&orbiters_projects::vcc_dir(), &path)))
                .map_err(|e| format!("{e:#}"));
            (path, r)
        });
    }
    fn poll_updates(&mut self, ctx: &egui::Context) {
        if let Some((path, result)) = self.updates_job.take() {
            match result {
                Ok(check) => self.updates = Some((path, check)),
                // A project that cannot be read now (moved, its manifest broken) shows no update.
                Err(_) => self.updates = None,
            }
        }
        // The repositories downloaded again once a window and every half hour while a project is shown, so new
        // versions show without Refresh; the check follows the view that brings them.
        if !self.fixture
            && self.selected.is_some()
            && !self.running()
            && !self.view_job.running()
            && !self.updates_job.running()
            && self.repos_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(30 * 60))
        {
            self.repos_checked = Some(Instant::now());
            self.load_view(ctx, true);
            self.view_quiet = true;
        }
    }
    /// The selected project's updates, with pre-releases when the switch is on.
    pub(super) fn updates_of(&self, path: &std::path::Path) -> Option<&updates::Updates> {
        self.updates.as_ref().filter(|(p, _)| p == path).map(|(_, c)| c.get(self.prerelease))
    }
    /// The server's catalog arrived: it replaces the suggestions and what was found among them.
    pub(super) fn set_catalog(&mut self, catalog: network::Catalog) {
        self.suggestions = orbiters_projects::suggestions::Suggestions::new(&catalog);
        // What is being looked at stays: this is not a new search.
        self.suggestions.set_query(&self.repo_query);
        self.catalog = catalog;
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
        self.server_now = Some(server(cx));
        self.poll_thumbs(ctx);
        let environment = crate::controller::vrchat_data::Scope::of(cx.snap);
        // Another account or VRChat session: every picture is asked for again, the ones shown staying until their
        // replacements arrive (a sign-in must not blank the page's covers first).
        // Only pictures not checked lately are asked for again (one missing for the account before is asked for).
        if !self.fixture && self.thumbnail_environment.as_ref() != Some(&environment) {
            self.thumbnail_environment = Some(environment);
            let fresh = &self.thumb_fresh;
            self.thumb_ids.retain(|id| fresh.contains(id));
            self.thumb_job = Job::default();
        }
        if let Some(rx) = &self.scan_rx {
            let mut done = false;
            while let Ok(event) = rx.try_recv() {
                match event {
                    // A list read before a change made here would undo it.
                    ScanEvent::Hidden(list) if self.hidden_changes == 0 => self.hidden = list,
                    ScanEvent::Hidden(_) => {}
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
                // Its scenes may have changed: their links are read again.
                self.load_links(ctx);
            }
        }
        if let Some(result) = self.view_job.take() {
            match result {
                Ok((view, templates, repo)) => {
                    if self.new_location.is_empty() {
                        self.new_location = view.default_path.display().to_string();
                    }
                    self.view = view;
                    self.templates = templates;
                    self.git = repo;
                    self.load_banners(ctx);
                    self.check_updates(ctx);
                }
                // The periodic look at the repositories says nothing when it fails: the cached view stays.
                Err(_) if self.view_quiet => {}
                Err(e) => self.error = Some(e),
            }
            self.view_quiet = false;
        }
        self.poll_updates(ctx);
        self.poll_editors(ctx, cx);
        self.poll_repair(ctx, cx);
        self.load_icons(ctx);
        self.load_repo_icons(ctx);
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
            let c = c.or_what_was_loaded(&self.catalog);
            self.set_catalog(c);
        }
        if let Some(result) = self.action.take() {
            self.action_label.clear();
            self.adding_repositories = false;
            self.busy_package = None;
            self.repo_adding = None;
            let change = self.last_change.take();
            if result.is_ok() {
                self.missing = None;
            }
            match result {
                Ok(Outcome::Changed(message)) => {
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message);
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
                }
                Ok(Outcome::Added(path, message)) => {
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message);
                    // The dialog that made it closes: the project opens in the panel.
                    if matches!(self.modal, Some(Modal::New | Modal::Duplicate(_))) {
                        self.close_modal(ctx);
                    }
                    self.reveal = Some(path);
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
                }
                Ok(Outcome::Said(message)) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, message),
                Ok(Outcome::Conflict(path, plan)) => {
                    self.conflict = Some(super::conflict::View { path, plan, solving: Job::default() });
                    let origin = self.anchor("conflict-origin");
                    self.open_modal(ctx, Modal::Conflict, origin);
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
                    // Packages no repository lists: the card offers to add the repositories that have them.
                    self.missing = super::missing_card::Card::from_error(&e, change.or_else(|| self.selected.clone().map(|p| (p, vpm::Change::Resolve))));
                    self.error = Some(e);
                    self.pending = None;
                    self.load_view(ctx, false);
                    self.refresh(ctx, false);
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
        // Not before this computer's copies are in: those checked lately are never asked for.
        if !self.fixture && self.started && !self.thumb_job.running() && self.thumb_preload.is_none() {
            let ids = self.pending_thumbnails();
            if !ids.is_empty() {
                self.thumb_ids.extend(ids.iter().cloned());
                let backend = cx.ctl.clone();
                let scope = crate::controller::vrchat_data::Scope::of(cx.snap);
                // A world's own picture (My World's thumbnail) is read here; the others are asked for.
                let local: HashMap<String, PathBuf> = self
                    .scan
                    .projects
                    .iter()
                    .flat_map(|p| &p.worlds)
                    .filter(|w| ids.contains(&w.id))
                    .filter_map(|w| Some((w.id.clone(), w.thumbnail.clone()?)))
                    .collect();
                // What is shown already: a picture that is the same is neither kept again nor shown again.
                let known: HashMap<String, u64> = ids.iter().filter_map(|id| Some((id.clone(), *self.thumb_sums.get(id)?))).collect();
                let versions = self.thumb_versions.clone();
                let requested_versions = versions.lock().unwrap().clone();
                self.thumb_job = Job::spawn(ctx, move || {
                    use crate::controller::vrchat_data::{Request, Response};
                    let mut images = HashMap::new();
                    let mut errors = Vec::new();
                    for (id, file) in &local {
                        if let Some(i) = image::open(file).ok().map(|i| i.thumbnail(640, 480).to_rgba8()) {
                            images.insert(id.clone(), ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], &i));
                        }
                    }
                    let (worlds, avatars): (Vec<String>, Vec<String>) =
                        ids.into_iter().filter(|id| !images.contains_key(id)).partition(|id| id.starts_with("wrld_"));
                    let requests =
                        [(!avatars.is_empty()).then_some(Request::Avatars { ids: avatars }), (!worlds.is_empty()).then_some(Request::Worlds { ids: worlds })];
                    for request in requests.into_iter().flatten() {
                        if let Response::Avatars { pictures, errors: failures } = backend.vrchat_data(scope.clone(), request)? {
                            errors.extend(failures);
                            for (id, p) in pictures {
                                images.insert(id, ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.rgba));
                            }
                        }
                    }
                    // Hold through disk writes: an upload cannot be overwritten by an older response.
                    let current = versions.lock().unwrap();
                    let mut same = Vec::new();
                    let images = images
                        .into_iter()
                        .filter_map(|(id, image): (String, ColorImage)| {
                            if current.get(&id) != requested_versions.get(&id) {
                                return None;
                            }
                            let sum = thumb_cache::sum(&image);
                            if known.get(&id) == Some(&sum) {
                                thumb_cache::touch(&id, sum);
                                same.push(id);
                                return None;
                            }
                            thumb_cache::save(&id, sum, &image);
                            Some((id, (image, sum)))
                        })
                        .collect();
                    Ok((images, same, errors, requested_versions))
                });
            }
        }
        if let Some(result) = self.thumb_job.take() {
            ctx.request_repaint(); // Publish this batch before requesting the next one.
            match result {
                Ok(batch) => self.apply_thumb_batch(ctx, batch),
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
            self.error = Some(tr!("Setup result was shown but could not be removed: {error}", error = e));
        }
        self.poll_panel(ctx, cx);
    }
    fn apply_thumb_batch(&mut self, ctx: &egui::Context, batch: ThumbBatch) {
        let (images, same, errors, requested_versions) = batch;
        let current = self.thumb_versions.lock().unwrap().clone();
        self.thumb_fresh.extend(same.into_iter().filter(|id| current.get(id) == requested_versions.get(id)));
        for (id, (image, sum)) in images {
            if current.get(&id) != requested_versions.get(&id) {
                continue;
            }
            self.thumb_fresh.insert(id.clone());
            self.set_thumb(ctx, id, image, sum);
        }
        if !errors.is_empty() {
            self.error = Some(orbiters_i18n::fill(
                &tr_n("{count} avatar picture could not load: {reason}. Refresh to retry.", "{count} avatar pictures could not load: {reason}. Refresh to retry.", errors.len() as u64),
                &[("reason", &errors[0])],
            ));
        }
    }
    fn pending_thumbnails(&self) -> Vec<String> {
        if self.thumb_preload.is_some() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        self.scan
            .projects
            .iter()
            .flat_map(|p| p.avatars.iter().chain(&p.worlds).map(|a| a.id.clone()))
            .filter(|id| !self.thumb_ids.contains(id) && seen.insert(id.clone()))
            .take(8)
            .collect()
    }
    /// Shows `image` (its sum `sum`) as the picture of `id`. Nothing changes when it is the picture shown already; a
    /// different one fades in over the one before ([`art::avatar`](super::art)), which stays drawn meanwhile.
    pub(super) fn set_thumb(&mut self, ctx: &egui::Context, id: String, image: ColorImage, sum: u64) {
        if self.thumb_sums.get(&id) == Some(&sum) && self.thumbs.contains_key(&id) {
            return;
        }
        let tex = ctx.load_texture(format!("project-avatar-{id}"), image, egui::TextureOptions::LINEAR);
        self.put_thumb(id, tex, sum);
    }
    /// [`Self::set_thumb`] with the texture made.
    pub(super) fn put_thumb(&mut self, id: String, tex: TextureHandle, sum: u64) {
        if self.thumb_sums.get(&id) == Some(&sum) && self.thumbs.contains_key(&id) {
            return;
        }
        self.thumb_sums.insert(id.clone(), sum);
        if let Some(before) = self.thumbs.insert(id.clone(), tex) {
            self.thumb_swaps += 1;
            self.thumb_before.insert(id, (before, self.thumb_swaps));
        }
    }
    /// How far the picture of `id` has faded in over the one before (1: no picture before, or done).
    pub(super) fn thumb_swap(ctx: &egui::Context, id: &str, serial: u64) -> f32 {
        orbiters_ui::anim::spring_from(ctx, Id::unique(("project-picture-swap", id, serial)), [0.0], [1.0], orbiters_ui::anim::Spring::SOFT)[0]
    }
    /// The pictures the last window kept, once read (those the page has meanwhile stay), and the fades that ended.
    fn poll_thumbs(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.thumb_preload {
            // The page's first visit waits for them (read on every core: a few tens of milliseconds) rather than
            // drawing empty covers that fill in after.
            let kept = if self.started && !self.thumb_preload_waited {
                self.thumb_preload_waited = true;
                rx.recv_timeout(Duration::from_secs(3)).map_err(|e| match e {
                    mpsc::RecvTimeoutError::Timeout => mpsc::TryRecvError::Empty,
                    mpsc::RecvTimeoutError::Disconnected => mpsc::TryRecvError::Disconnected,
                })
            } else {
                rx.try_recv()
            };
            match kept {
                Ok(kept) => {
                    self.thumb_preload = None;
                    if !self.fixture {
                        for c in kept {
                            if c.fresh {
                                self.thumb_fresh.insert(c.id.clone());
                                self.thumb_ids.insert(c.id.clone());
                            }
                            if !self.thumbs.contains_key(&c.id) {
                                let tex = ctx.load_texture(format!("project-avatar-{}", c.id), c.image, egui::TextureOptions::LINEAR);
                                self.thumbs.insert(c.id.clone(), tex);
                                self.thumb_sums.insert(c.id.clone(), c.sum);
                                self.thumb_instant.insert(c.id);
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(30)),
                Err(mpsc::TryRecvError::Disconnected) => self.thumb_preload = None,
            }
        }
        self.thumb_before.retain(|id, (_, serial)| Self::thumb_swap(ctx, id, *serial) < 1.0);
    }
    /// The panel's own jobs: projects' sizes, the Blender links, the worlds' captures and pictures, files picked to
    /// add to a world, and the Duplicate dialog's check.
    fn poll_panel(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some((path, lookup)) = self.size_lookup.take() {
            if let Some(s) = lookup.size {
                self.sizes.insert(path.clone(), s);
            }
            if !lookup.fresh && !self.size_waiting.contains(&path) {
                self.size_waiting.push(path);
            }
        }
        if let Some((path, result)) = self.size_walk.take() {
            self.size_waiting.retain(|p| p != &path);
            match result {
                Ok(s) => {
                    self.sizes.insert(path, s);
                }
                // Measuring is a nicety: a project it fails on shows no size (the folder went away, say).
                Err(e) => eprintln!("project size {}: {e}", path.display()),
            }
        }
        // The first waiting is walked; it stays listed (as measuring) until its answer arrives.
        if let Some(path) = self.size_waiting.first().cloned().filter(|_| !self.size_walk.running()) {
            let cancel = self.size_cancel.clone();
            self.size_walk = Job::spawn(ctx, move || {
                discovery::low_priority();
                let r = size::measure(&path, &cancel).map_err(|e| format!("{e:#}"));
                (path, r)
            });
        }
        if let Some((path, links)) = self.links_job.take() {
            if self.selected.as_ref() == Some(&path) {
                self.links = links;
                self.links_of = Some(path);
            }
        }
        if let Some(here) = self.here_job.take() {
            self.here = here;
        }
        if let Some(Some((id, name, paths))) = self.pick_job.take() {
            if !paths.is_empty() {
                self.handed.insert(id.clone(), (orbiters_ui::anim::time(ctx), crate::state::UploadKind::Pictures));
                cx.ctl.upload_pictures(id, name, paths);
            }
        }
        if let Some(checked) = self.dup.check.take() {
            self.dup.checked = Some(checked);
        }
    }
    /// Looks up (then, when it changed or is old, measures) the size of the project in `path`, once per visit of the
    /// page.
    pub(super) fn want_size(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        if self.fixture || !self.sizes_asked.insert(path.to_path_buf()) {
            return;
        }
        if self.size_lookup.running() {
            // One lookup at a time: this one waits for the next frame.
            self.sizes_asked.remove(path);
            return;
        }
        let p = path.to_path_buf();
        self.size_lookup = Job::spawn(ctx, move || {
            let lookup = size::lookup(&p);
            (p, lookup)
        });
    }
    /// Whether the size of `path` is being looked up or measured (or waits to be).
    pub(super) fn measuring(&self, path: &std::path::Path) -> bool {
        self.size_waiting.iter().any(|p| p == path) || (self.size_lookup.running() && self.sizes_asked.contains(path) && !self.sizes.contains_key(path))
    }
    /// Whether the project in `path` is hidden from the lists.
    pub(super) fn is_hidden(&self, path: &std::path::Path) -> bool {
        hidden::contains(&self.hidden, path)
    }
    /// Hides the project in `path` from the lists, or shows it again, and keeps that on a worker.
    pub(super) fn set_hidden(&mut self, path: &std::path::Path, hide: bool) {
        let key = discovery::key(path);
        let changed = if hide { self.hidden.insert(key) } else { self.hidden.remove(&key) };
        if !changed || self.fixture {
            return;
        }
        self.hidden_changes += 1;
        let (list, generation) = (self.hidden.clone(), self.hidden_changes);
        std::thread::spawn(move || {
            if let Err(e) = hidden::save(&list, generation) {
                eprintln!("hidden projects: {e:#}");
            }
        });
    }
    /// The selected project's Blender links, read again.
    pub(super) fn load_links(&mut self, ctx: &egui::Context) {
        if self.fixture {
            return;
        }
        let Some(project) = self.selected.as_ref().and_then(|s| self.scan.projects.iter().find(|p| &p.path == s)) else { return };
        let path = project.path.clone();
        let scenes: Vec<String> = project.avatars.iter().chain(&project.worlds).map(|a| a.scene.clone()).collect();
        self.links_job = Job::spawn(ctx, move || {
            let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
            let links = blender::links(&path, &scenes);
            (path, links)
        });
    }
    /// What this member added to Orbiters' Map explorer (a stand-in in screenshots).
    pub(super) fn maps(&self, cx: &Cx) -> Option<crate::state::MapsMine> {
        if self.fixture {
            self.fixture_maps.clone()
        } else {
            cx.ctl.maps()
        }
    }
    /// The uploads to Orbiters' Map explorer of the world `id`, the latest first.
    pub(super) fn uploads(&self, cx: &Cx, id: &str) -> Vec<crate::state::UploadStatus> {
        let all = if self.fixture { &self.fixture_uploads } else { &cx.snap.uploads };
        let mut list: Vec<_> = all.iter().filter(|u| u.world_id == id).cloned().collect();
        list.sort_by_key(|u| std::cmp::Reverse(u.id));
        list
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
    /// Looks for the icons of the repositories drawn since the last look (the dialog's rows and its suggestions), as
    /// packages' icons are looked for.
    fn load_repo_icons(&mut self, ctx: &egui::Context) {
        let wanted = std::mem::take(&mut self.repo_icons_wanted);
        if (!self.fixture || self.fixture_live_icons) && !self.repo_icon_job.running() {
            let mut batch: Vec<(String, Vec<String>)> = vec![];
            for url in wanted {
                if batch.len() >= ICON_BATCH || !self.repo_icons_asked.insert(url.clone()) {
                    continue;
                }
                // Its packages' authors' websites, the most common first.
                let mut counts: HashMap<&String, usize> = HashMap::new();
                for p in self.view.packages.iter().filter(|p| p.repository.as_deref() == Some(url.as_str())) {
                    if let Some(a) = &p.author_url {
                        *counts.entry(a).or_default() += 1;
                    }
                }
                let mut authors: Vec<(&String, usize)> = counts.into_iter().collect();
                authors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                let authors = authors.into_iter().take(2).map(|(a, _)| a.clone()).collect();
                batch.push((url, authors));
            }
            if !batch.is_empty() {
                self.repo_icon_job = Job::spawn(ctx, move || {
                    let sources: Vec<icons::Sources> = batch.iter().map(|(url, authors)| icons::repository_sources(url, authors)).collect();
                    batch.into_iter().map(|(url, _)| url).zip(icons::resolve_all(&sources)).collect()
                });
            }
        }
        if let Some(found) = self.repo_icon_job.take() {
            for (url, image) in found {
                if let Some(image) = image {
                    let icon = PackageIcon::load(ctx, &format!("repo-{url}"), &image);
                    self.repo_icons.insert(url, icon);
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
        self.task(ctx, tr("Adding Mixer…"), move || {
            orbiters_projects::runtime()?.block_on(vpm::install_orbiters(&orbiters_projects::vcc_dir(), &path, &[MIXER]))?;
            Ok(Outcome::Changed(tr("Mixer is in the project: open it in Unity to try it").into()))
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
            self.error = Some(tr("N4N0 has not been published on the production gallery yet. My Avatar can still be installed.").into());
            return;
        }
        self.open_modal(ctx, Modal::Setup, origin);
        self.flow = None;
        self.pending = Some((project.name.clone(), avatar.name.clone(), nano));
        self.task(ctx, if nano { tr("Installing My Avatar and preparing N4N0…") } else { tr("Installing My Avatar…") }, move || {
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
        let names = ["MasculineCanine", "Rexouium", "Nova", "Ultinova", "Kitsune", "Protogen"]; // i18n: not shown (sample data of the screenshot scenes)
        let projects = [
            ("My avatars", 4, discovery::Kind::Avatars, 0.2), // i18n: not shown (sample data of the screenshot scenes)
            ("Weekend avatar", 1, discovery::Kind::Avatars, 26.0), // i18n: not shown (sample data of the screenshot scenes)
            ("Moonlight world", 0, discovery::Kind::Worlds, 50.0), // i18n: not shown (sample data of the screenshot scenes)
            ("Club outfits", 2, discovery::Kind::Avatars, 90.0), // i18n: not shown (sample data of the screenshot scenes)
            ("Avatar backup", 1, discovery::Kind::Avatars, 200.0), // i18n: not shown (sample data of the screenshot scenes)
            ("Old avatar project", 3, discovery::Kind::Avatars, 400.0), // i18n: not shown (sample data of the screenshot scenes)
            ("Test world", 0, discovery::Kind::Worlds, 700.0), // i18n: not shown (sample data of the screenshot scenes)
        ];
        let found = if name == "projects-proposals" { 14 } else { 4 };
        self.scan.projects = (0..3 + found)
            .map(|i| {
                let (label, count, kind, hours) = projects[i.min(projects.len() - 1)];
                let label = if i < projects.len() { label.to_string() } else { format!("Avatar project {}", i - 2) }; // i18n: not shown (sample data of the screenshot scenes)
                let avatars = (0..count)
                    .map(|a| {
                        let k = (a + i) % names.len();
                        Avatar {
                            id: format!("avtr_00000000-0000-4000-8000-{:012}", k),
                            name: names[k].into(),
                            scene: format!("Assets/{}/{}.unity", names[k], if a == 0 { "Avatar" } else { "Scenes/Main" }), // i18n: not shown (sample data of the screenshot scenes)
                            path: names[k].into(),
                            ..Default::default()
                        }
                    })
                    .collect();
                Project {
                    path: PathBuf::from(format!("{STAND_IN_PROJECTS}/{label}")),
                    name: label,
                    unity: if i == 5 { "2019.4.31f1".into() } else { "2022.3.22f1".into() },
                    kind,
                    modified: discovery::now() - (hours * 3600.0) as u64 - i as u64 * 7,
                    registered: i < 3,
                    missing: false,
                    avatars,
                    worlds: vec![],
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
            let id = format!("avtr_00000000-0000-4000-8000-{:012}", i);
            let image = ColorImage::from_rgb([w, h], &pixels);
            let sum = thumb_cache::sum(&image);
            let tex = ctx.load_texture(*n, image, egui::TextureOptions::LINEAR);
            if name == "projects-revisit" {
                // Asked for again: the same picture, but Nova's, changed on VRChat meanwhile (its colours turned).
                if i == 2 {
                    let turned: Vec<u8> = pixels.chunks_exact(3).flat_map(|c| [c[2], c[0], c[1]]).collect();
                    let image = ColorImage::from_rgb([w, h], &turned);
                    let sum = thumb_cache::sum(&image);
                    self.fixture_arrivals.push((id.clone(), ctx.load_texture("Nova changed", image, egui::TextureOptions::LINEAR), sum));
                } else {
                    self.fixture_arrivals.push((id.clone(), tex.clone(), sum));
                }
            }
            self.thumb_sums.insert(id.clone(), sum);
            self.thumbs.insert(id, tex);
        }
        self.view.repos = vec![
            vpm::Repository {
                name: "VRChat official".into(), // i18n: not shown (sample data of the screenshot scenes)
                url: "https://packages.vrchat.com/official".into(),
                id: "official".into(),
                enabled: true,
                builtin: true,
            },
            vpm::Repository {
                name: "VRChat curated".into(), // i18n: not shown (sample data of the screenshot scenes)
                url: "https://packages.vrchat.com/curated".into(),
                id: "curated".into(),
                enabled: false,
                builtin: true,
            },
            vpm::Repository { name: "Orbiters".into(), url: vpm::ORBITERS_REPOSITORY.into(), id: "orbiters".into(), enabled: true, builtin: false },
        ];
        self.view.packages = [
            ("com.vrchat.avatars", "VRChat SDK – Avatars", Some("3.9.0"), vec!["3.9.0", "3.8.2", "3.8.1", "3.7.6"]), // i18n: not shown (sample data of the screenshot scenes)
            ("com.vrchat.base", "VRChat SDK – Base", Some("3.9.0"), vec!["3.9.0", "3.8.2", "3.8.1", "3.7.6"]), // i18n: not shown (sample data of the screenshot scenes)
            ("orbiters.toolkit", "Orbiters Toolkit", Some("0.5.0"), vec!["0.6.0-beta.2", "0.5.0", "0.4.5"]),
            ("orbiters.myavatar", "My Avatar", Some("0.9.14"), vec!["0.9.14", "0.9.12", "0.8.0"]),
            ("com.vrcfury.vrcfury", "VRCFury", Some("1.1434.0"), vec!["1.1434.0", "1.1300.0"]),
            ("orbiters.mixer", "Mixer", None, vec!["0.7.0", "0.6.2"]),
            ("orbiters.refit", "ReFit", None, vec!["0.7.3", "0.7.2"]),
            ("com.poiyomi.toon", "Poiyomi Toon Shader", None, vec!["9.2.67"]), // i18n: not shown (sample data of the screenshot scenes)
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
            template(tr("VRChat Avatar Project"), "com.vrchat.template.avatar", "2022.3.22f1", true),
            template(tr("VRChat World Project"), "com.vrchat.template.world", "2022.3.22f1", true),
            template("Studio base", "com.studio.template.base", "", false), // i18n: not shown (sample data of the screenshot scenes)
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
        for (url, look) in [
            ("https://packages.vrchat.com/official", Look::Picture([30., 90., 200.])),
            (vpm::ORBITERS_REPOSITORY, Look::Wings),
            ("https://vcc.vrcfury.com", Look::Mark([24., 24., 28.])),
            ("https://poiyomi.github.io/vpm/index.json", Look::Mark([255., 120., 190.])),
        ] {
            self.repo_icons.insert(url.into(), stand_in_icon(ctx, &format!("repo-{url}"), look));
        }
        self.new_location = STAND_IN_PROJECTS.into();
        self.new_name = "My new avatar".into(); // i18n: not shown (sample data of the screenshot scenes)
        let known = vec![
            network::Listing {
                name: "VRCFury".into(),
                url: "https://vcc.vrcfury.com".into(),
                description: "Avatar tools and setup helpers".into(), // i18n: not shown (sample data of the screenshot scenes)
                ..Default::default()
            },
            network::Listing {
                name: "Poiyomi".into(), // i18n: not shown (sample data of the screenshot scenes)
                url: "https://poiyomi.github.io/vpm/index.json".into(),
                description: "Poiyomi Toon and Pro shaders".into(), // i18n: not shown (sample data of the screenshot scenes)
                ..Default::default()
            },
        ];
        let public = vec![network::Listing {
            name: "Orbiters".into(),
            url: vpm::ORBITERS_REPOSITORY.into(),
            description: "My Avatar, Toolkit, Mixer and more".into(), // i18n: not shown (sample data of the screenshot scenes)
            ..Default::default()
        }];
        self.set_catalog(network::Catalog { known, public, errors: vec![] });
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
        let now = discovery::now();
        let commit = |short: &str, author: &str, ago: u64, subject: &str, body: &str| git::Commit {
            hash: format!("{short}{}", "0".repeat(33)),
            short: short.into(),
            author: author.into(),
            email: format!("{}@example.com", author.to_lowercase()),
            time: now - ago,
            subject: subject.into(),
            body: body.into(),
        };
        self.git = Some(git::Repo {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead: 2,
            behind: 0,
            changes: 3,
            commits: vec![
                commit(
                    "8e21f0a",
                    "Enzo", // i18n: not shown (sample data of the screenshot scenes)
                    2 * 3600,
                    "Save MasculineCanine's outfit with the new hoodie and its expression menu", // i18n: not shown (sample data of the screenshot scenes)
                    "The hoodie's materials moved to Poiyomi 9.2, its toggles to the expression menu.\nChecked in Build & Test.", // i18n: not shown (sample data of the screenshot scenes)
                ),
                commit("620df02", "Enzo", 26 * 3600, "Add the expression menu", ""), // i18n: not shown (sample data of the screenshot scenes)
                commit("1b9c4d3", "Nova", 3 * 86400, "Texture pass on the hoodie", "Albedo and normal maps redone at 2K."), // i18n: not shown (sample data of the screenshot scenes)
                commit("77aa310", "Enzo", 9 * 86400, "Import the base avatar", ""), // i18n: not shown (sample data of the screenshot scenes)
            ],
            web: Some("https://github.com/Orbiters-cc/my-avatars".into()),
            host: Some("github.com".into()),
        });
        // The world project's own thumbnail (My World's), a stand-in.
        if let Some(w) = self.scan.projects.iter_mut().find(|p| p.kind == discovery::Kind::Worlds) {
            w.worlds = vec![Avatar { id: "wrld_00000000-0000-4000-8000-000000000001".into(), name: "Moonlight".into(), ..Default::default() }]; // i18n: not shown (sample data of the screenshot scenes)
            let (wd, ht) = (480usize, 360usize);
            let mut pixels = vec![0u8; wd * ht * 3];
            for y in 0..ht {
                for x in 0..wd {
                    let (fx, fy) = (x as f32 / wd as f32, y as f32 / ht as f32);
                    let moon = ((fx - 0.72).powi(2) + (fy - 0.28).powi(2)).sqrt() < 0.09;
                    let hill = fy > 0.68 + 0.08 * (fx * 6.0).sin() * (1.0 - fx);
                    let sky = [20.0 + 60.0 * fy, 24.0 + 40.0 * fy, 60.0 + 90.0 * fy];
                    let c = if moon {
                        [235.0, 232.0, 210.0]
                    } else if hill {
                        [16.0, 22.0 + 30.0 * fy, 30.0]
                    } else {
                        sky
                    };
                    for k in 0..3 {
                        pixels[(y * wd + x) * 3 + k] = c[k] as u8;
                    }
                }
            }
            self.thumbs
                .insert(w.worlds[0].id.clone(), ctx.load_texture("fixture-world", ColorImage::from_rgb([wd, ht], &pixels), egui::TextureOptions::LINEAR));
        }
        // A project whose avatars have no picture but one (Enzo's MCB Test): the cover shows that one alone.
        if let Some(p) = self.scan.projects.get_mut(5) {
            for (k, a) in p.avatars.iter_mut().enumerate().skip(1) {
                a.id = format!("avtr_00000000-0000-4000-8000-1000000000{k:02}");
            }
        }
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
                self.git = None;
                self.fixture_view_loading = true;
            }
            "projects-error" => {
                self.error = Some("VCC's project list could not be read: settings.json is locked by another program. Close VCC and refresh.".into()); // i18n: not shown (sample data of the screenshot scenes)
                self.scan.warnings = vec!["D: could not be searched: access denied".into()]; // i18n: not shown (sample data of the screenshot scenes)
            }
            "projects-busy" => {
                self.selected = Some(first.path.clone());
                self.fixture_action = Some(tr!("Duplicating {name}…", name = SAMPLE_PROJECT));
                *self.copy_progress.lock().unwrap() = templates::Progress {
                    copied: 1840,
                    total: 4310,
                    file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                    phase: String::from(mark("Copying project")),
                };
            }
            "projects-search" => self.query = "avatar".into(),
            "projects-avatar-menu" => self.selected = Some(first.path.clone()),
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
                self.fixture_action = Some(tr("Adding Mixer…").into());
            }
            "projects-mixer-open-mid" | "projects-mixer-open" | "projects-mixer-fold-mid" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_mixer = true;
            }
            "projects-repos" => self.modal = Some(Modal::Repos),
            "projects-repos-cards"
            | "projects-repos-scrolled"
            | "projects-repos-search"
            | "projects-repos-search-japanese"
            | "projects-repos-search-owner"
            | "projects-repos-nothing"
            | "projects-repos-many"
            | "projects-repos-added"
            | "projects-repos-unreachable"
            | "projects-repos-partly"
            | "projects-repos-bare" => self.stage_repo_cards(ctx, name),
            "projects-panel-cut" | "projects-panel-cut-top" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_packages = name == "projects-panel-cut";
            }
            "projects-repos-crossfade" => {
                self.modal = Some(Modal::Repos);
                self.fixture_repo_icons_later = std::mem::take(&mut self.repo_icons);
            }
            "projects-repos-live" => {
                // The repositories' real websites, their icons looked for as the dialog does.
                self.modal = Some(Modal::Repos);
                self.repo_icons.clear();
                self.fixture_live_icons = true;
            }
            "projects-commits" | "projects-commits-mid" | "projects-commits-open" | "projects-commits-narrow" => {
                self.selected = Some(first.path.clone());
                self.scroll_to_commits = true;
                if name == "projects-commits-open" {
                    let hash = self.git.as_ref().map(|g| g.commits[0].hash.clone()).unwrap_or_default();
                    self.commit_open.insert(hash);
                }
            }
            "projects-world" => {
                let world = self.scan.projects.iter().find(|p| p.kind == discovery::Kind::Worlds).map(|p| p.path.clone());
                self.selected = world;
                self.git = None;
            }
            "projects-repos-loading" => {
                self.modal = Some(Modal::Repos);
                self.set_catalog(network::Catalog::default());
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
                self.new_name = "My avatars copy".into(); // i18n: not shown (sample data of the screenshot scenes)
            }
            "projects-new-copying" => {
                self.modal = Some(Modal::New);
                self.duplicate = true;
                self.new_name = "My avatars copy".into(); // i18n: not shown (sample data of the screenshot scenes)
                self.fixture_action = Some(tr("Creating project…").into());
                *self.copy_progress.lock().unwrap() = templates::Progress {
                    copied: 1840,
                    total: 4310,
                    file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                    phase: String::from(mark("Copying project")),
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
                self.fixture_action = Some(tr("Installing My Avatar and preparing N4N0…").into());
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
                self.flow = Some(flow(true, Some((setup::Status::Done, "N4N0 was added to MasculineCanine and the scene was saved.")), false)); // i18n: not shown (sample data of the screenshot scenes)
            }
            "projects-setup-declined" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(true, Some((setup::Status::Declined, "You chose Not now in Unity. Nothing was changed.")), false)); // i18n: not shown (sample data of the screenshot scenes)
            }
            "projects-setup-failed" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(
                    true,
                    Some((setup::Status::Failed, "The avatar's descriptor could not be found in Assets/MasculineCanine/Avatar.unity.")), // i18n: not shown (sample data of the screenshot scenes)
                    false,
                ));
            }
            "projects-installed" => {
                self.modal = Some(Modal::Setup);
                self.flow = Some(flow(false, None, true));
            }
            _ => {}
        }
        self.stage_panel(name);
        super::vrchat_picture::stage(self, name);
        if self.modal.is_some() {
            self.modal_serial = 1;
            self.modal_origin = Rect::from_center_size(ctx.content_rect().center() + egui::vec2(0.0, 120.0), egui::vec2(180.0, 44.0));
        }
    }
    /// The stand-ins of the panel's sizes, Blender links, worlds' captures and uploads, the Duplicate dialog and the
    /// hidden projects (`screenshot/projects_panel.rs`).
    fn stage_panel(&mut self, name: &str) {
        self.stage_packages(name);
        self.stage_storage(name);
        use crate::state::{MapsMine, UploadKind, UploadState, UploadStatus};
        use orbiters_core::maps::{MinePicture, MinePreview, MineWorld};
        // Every project measured (its Library a good part of it), but the one the panel measures in `-measuring`.
        for (i, p) in self.scan.projects.iter().enumerate() {
            let gb = [18.4, 3.1, 7.6, 12.9, 2.2, 5.4, 9.8][i % 7];
            let total = (gb * 1e9) as u64;
            self.sizes.insert(p.path.clone(), size::Size { total, library: total / 10 * 6, files: 40_000 + 23_000 * i as u64 });
        }
        let first = self.scan.projects[0].path.clone();
        let world = self.scan.projects.iter().find(|p| p.kind == discovery::Kind::Worlds).map(|p| p.path.clone());
        if let Some(w) = self.scan.projects.iter_mut().find(|p| p.kind == discovery::Kind::Worlds) {
            for a in &mut w.worlds {
                a.scene = "Assets/Scenes/Moonlight.unity".into();
            }
        }
        let world_id = "wrld_00000000-0000-4000-8000-000000000001".to_string();
        let now = discovery::now();
        let links = |project: &str| -> Vec<blender::Link> {
            let at = |file: &str| PathBuf::from(format!("{STAND_IN_PROJECTS}/{project}/{file}"));
            match project {
                "My avatars" => vec![ // i18n: not shown (sample data of the screenshot scenes)
                    blender::Link::new(
                        at("Assets/MCB/blenderProjects/MasculineCanine_a4d47dec0b66/blend~/MasculineCanine.blend"),
                        blender::Tool::Mcb,
                        Some("Assets/MasculineCanine/Avatar.unity".into()),
                        Some("MasculineCanine".into()),
                        now - 3 * 3600,
                    ),
                    blender::Link::new(
                        PathBuf::from(if cfg!(target_os = "macos") { "/Volumes/Archive/Blender/Hoodie outfit.blend" } else { "H:/Blender/Hoodie outfit.blend" }),
                        blender::Tool::Mixer,
                        Some("Assets/MasculineCanine/Avatar.unity".into()),
                        None,
                        now - 2 * 86400,
                    ),
                ],
                _ => vec![blender::Link::new(
                    at("Assets/Orbiters/Mixer/Blender/0b4e0f0e2c8d4a51b6f0a4c7d1e2f3a4/blend~/Moonlight.blend"),
                    blender::Tool::Mixer,
                    Some("Assets/Scenes/Moonlight.unity".into()),
                    None,
                    now - 26 * 3600,
                )],
            }
        };
        let here = super::world_card::Here {
            name: Some("Moonlight".into()), // i18n: not shown (sample data of the screenshot scenes)
            captures: vec![
                super::world_card::Capture { dir: PathBuf::from("captures/moonlight-2"), when: "28 Oct 2026, 21:14".into(), splats: Some(1_240_000) }, // i18n: not shown (sample data of the screenshot scenes)
                super::world_card::Capture { dir: PathBuf::from("captures/moonlight-1"), when: "21 Oct 2026, 19:02".into(), splats: Some(860_000) }, // i18n: not shown (sample data of the screenshot scenes)
                super::world_card::Capture { dir: PathBuf::from("captures/moonlight-0"), when: "20 Oct 2026, 22:40".into(), splats: None }, // i18n: not shown (sample data of the screenshot scenes)
            ],
            pictures: 23,
            folder: None,
        };
        let mine = |preview: bool, pictures: u64| MapsMine {
            worlds: vec![MineWorld {
                world_id: world_id.clone(),
                preview: preview.then(|| MinePreview { id: 1, splats: 1_240_000, ..Default::default() }),
                pictures: (0..pictures).map(|id| MinePicture { id, source_hash: format!("{id:064x}") }).collect(),
            }],
            error: None,
        };
        let upload = |state: UploadState| UploadStatus {
            id: 7,
            world_id: world_id.clone(),
            world_name: "Moonlight".into(), // i18n: not shown (sample data of the screenshot scenes)
            kind: UploadKind::Pictures,
            paths: (0..4).map(|i| PathBuf::from(format!("VRChat/2026-10/VRChat_2026-10-28_21-1{i}.png"))).collect(),
            sent: 45,
            total: 100,
            state,
        };
        match name {
            "projects-size" | "projects-size-measuring" => {
                self.selected = Some(first.clone());
                if name == "projects-size-measuring" {
                    self.sizes.remove(&first);
                }
            }
            "projects-avatars-links" => {
                self.selected = Some(first.clone());
                self.links = links("My avatars"); // i18n: not shown (sample data of the screenshot scenes)
                self.links_of = Some(first.clone());
            }
            "projects-worlds"
            | "projects-worlds-sharing"
            | "projects-worlds-done"
            | "projects-worlds-failed"
            | "projects-worlds-new"
            | "projects-worlds-signed-out"
            | "projects-worlds-capture-menu" => {
                self.selected = world.clone();
                self.git = None;
                self.links = links("Moonlight world"); // i18n: not shown (sample data of the screenshot scenes)
                self.links_of = world.clone();
                self.here.insert(world_id.clone(), here);
                self.fixture_maps = Some(if name == "projects-worlds-new" { MapsMine::default() } else { mine(true, 12) });
                self.fixture_uploads = match name {
                    "projects-worlds-sharing" => vec![upload(UploadState::Sending)],
                    "projects-worlds-done" => vec![upload(UploadState::Done { added: 4, skipped: 1 })],
                    "projects-worlds-failed" => vec![upload(UploadState::Failed { message: "Orbiters could not be reached. Try again shortly.".into() })], // i18n: not shown (sample data of the screenshot scenes)
                    _ => vec![],
                };
            }
            "projects-duplicate" | "projects-duplicate-taken" | "projects-duplicate-library" | "projects-duplicate-copying" => {
                self.selected = Some(first.clone());
                self.modal = Some(Modal::Duplicate(first.clone()));
                let copy = if name == "projects-duplicate-taken" { "Weekend avatar" } else { "My avatars copy" }; // i18n: not shown (sample data of the screenshot scenes)
                self.dup = Dup {
                    name: copy.into(),
                    location: STAND_IN_PROJECTS.into(),
                    library: name == "projects-duplicate-library",
                    checked: Some((STAND_IN_PROJECTS.into(), copy.into(), name == "projects-duplicate-taken", Some(412_000_000_000))),
                    ..Default::default()
                };
                if name == "projects-duplicate-copying" {
                    self.fixture_action = Some(tr!("Duplicating {name}…", name = SAMPLE_PROJECT));
                    *self.copy_progress.lock().unwrap() = templates::Progress {
                        copied: 1840,
                        total: 4310,
                        file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                        phase: String::from(mark("Copying project")),
                    };
                }
            }
            "projects-hidden" | "projects-hidden-shown" | "projects-hidden-press" => {
                for p in [1, 4] {
                    if let Some(p) = self.scan.projects.get(p) {
                        self.hidden.insert(discovery::key(&p.path));
                    }
                }
                self.show_hidden = name == "projects-hidden-shown";
            }
            "projects-header-blur" => {
                self.selected = Some(first.clone());
                self.scroll_gallery = Some(250.0);
            }
            _ => {}
        }
    }
    /// Screenshots: what the person does at `frame` of the scenario `name` (a press on a control where it was drawn,
    /// the pointer over a card), for states reached through the interface and frames in the middle of their motion.
    pub fn stage_frame(&mut self, name: &str, frame: u32) -> Option<Poke> {
        if let Some(poke) = self.stage_storage_frame(name, frame) {
            return Some(poke);
        }
        // The page left (frame 50) and entered again (62, the harness), then what a new VRChat session does (80): the
        // scan's list again and every picture asked for again, as `poll` takes them.
        // Right-click menus: a project card's, a found project's, an avatar's row.
        match (name, frame) {
            ("projects-card-menu", 40) => return self.anchors.get("card-1").map(|r| Poke::Context(r.center())),
            ("projects-unity-menu", 40) => return self.anchors.get("card-0").map(|r| Poke::Context(r.center())),
            ("projects-avatar-menu", 50) => return self.anchors.get("vrchat-picture-0").map(|r| Poke::Context(r.center() + egui::vec2(220.0, 0.0))),
            _ => {}
        }
        if name == "projects-revisit" && frame == 80 {
            self.scan = self.scan.clone();
            self.thumb_ids.clear();
            for (id, tex, sum) in std::mem::take(&mut self.fixture_arrivals) {
                self.put_thumb(id, tex, sum);
            }
        }
        if name == "projects-crossfade" && frame == 60 {
            self.thumbs = std::mem::take(&mut self.fixture_later);
            self.icons = std::mem::take(&mut self.fixture_icons_later);
        }
        self.stage_repo_query(name, frame);
        if name == "projects-repos-crossfade" && frame == 60 {
            self.repo_icons = std::mem::take(&mut self.fixture_repo_icons_later);
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
        if name.starts_with("projects-vrchat-picture") {
            return super::vrchat_picture::stage_frame(self, name, frame);
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
            ("projects-worlds-capture-menu", 60) => at("world-capture-0").map(Poke::Press),
            ("projects-hidden-press", 60) => at("hidden-filter").map(Poke::Press),
            _ => None,
        }
    }
}
/// How a stand-in package icon looks (screenshots).
#[derive(Clone, Copy)]
pub(super) enum Look {
    /// A picture filling its tile: a gradient in this colour and a white speech bubble.
    Picture([f32; 3]),
    /// Orbiters' wings on a dark square, as orbiters.cc's icon.
    Wings,
    /// A diamond in this colour on nothing (a mark to frame).
    Mark([f32; 3]),
}

/// Where screenshots' stand-in projects say they are.
const SAMPLE_PROJECT: &str = "My avatars"; // i18n: not shown (the sample project of the screenshot scenes)
pub(super) const STAND_IN_PROJECTS: &str = if cfg!(target_os = "macos") { "/Users/enzo/Unity projects" } else { "H:/Unity projects" }; // i18n: not shown (sample data of the screenshot scenes)

pub(super) fn stand_in_icon(ctx: &egui::Context, name: &str, look: Look) -> PackageIcon {
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
    /// A right press there (a context menu), released on the next frame.
    Context(egui::Pos2),
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;

    #[test]
    fn projects_upload_rejects_an_older_refresh_result() {
        let mut state = State::default();
        let ctx = egui::Context::default();
        let id = "avtr_upload".to_string();
        let new = ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4]);
        state.thumb_versions.lock().unwrap().insert(id.clone(), 1);
        state.set_thumb(&ctx, id.clone(), new, 123);
        let texture = state.thumbs[&id].id();
        let old = ColorImage::new([2, 2], vec![egui::Color32::RED; 4]);
        state.apply_thumb_batch(&ctx, (HashMap::from([(id.clone(), (old, 456))]), vec![], vec![], HashMap::new()));
        assert_eq!(state.thumbs[&id].id(), texture);
        assert_eq!(state.thumb_sums[&id], 123);
    }

    #[test]
    fn projects_preload_timeout_is_only_waited_once() {
        let mut state = State::default();
        state.started = true;
        state.thumb_preload_waited = true;
        let (_tx, rx) = mpsc::channel();
        state.thumb_preload = Some(rx);
        let start = Instant::now();
        state.poll_thumbs(&egui::Context::default());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(state.thumb_preload.is_some());
    }

    #[test]
    fn projects_window_cache_is_present_before_requests_and_unchanged_keeps_texture() {
        let dir = std::env::temp_dir().join(format!("orbiters-projects-state-{}", std::process::id()));
        let id = "avtr_cache_test".to_string();
        let image = ColorImage::new([8, 8], vec![egui::Color32::RED; 64]);
        let sum = thumb_cache::sum(&image);
        thumb_cache::save_in(&dir, &id, sum, &image).unwrap();
        let mut state = State::default();
        state.started = true;
        state.scan.projects.push(Project { avatars: vec![Avatar { id: id.clone(), ..Default::default() }], ..Default::default() });
        let (tx, rx) = mpsc::channel();
        state.thumb_preload = Some(rx);
        assert!(state.pending_thumbnails().is_empty(), "no requests while preload runs");
        tx.send(thumb_cache::load_in(&dir)).unwrap();
        let ctx = egui::Context::default();
        state.poll_thumbs(&ctx);
        assert!(state.thumbs.contains_key(&id), "present on first poll");
        assert!(state.thumb_instant.contains(&id));
        assert!(state.pending_thumbnails().is_empty(), "fresh IDs must never reach the request job");
        let fresh = &state.thumb_fresh;
        state.thumb_ids.retain(|id| fresh.contains(id));
        assert!(state.pending_thumbnails().is_empty(), "scope change retains fresh IDs");
        let texture = state.thumbs[&id].id();
        state.set_thumb(&ctx, id.clone(), image, sum);
        assert_eq!(texture, state.thumbs[&id].id());
        assert!(state.thumb_before.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
