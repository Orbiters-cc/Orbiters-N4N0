# Unity projects and avatar setup

The **Unity projects** page, immediately after Capture, manages the VCC/ALCOM project list and VPM packages without
requiring Creator Companion to run. **Capture → Add to my avatar** uses the same project and avatar inventory.

`crates/orbiters-projects` owns discovery, Unity text-scene inspection, VPM operations, templates, thumbnails, the
setup-request contract, projects' sizes (`size`), the hidden list (`hidden`) and Blender links (`blender`). The
desktop's `pages/projects` holds the window's background jobs and their snapshots. Disk walks, file dialogs, Git,
package resolution, copies, thumbnail decoding and HTTP requests run off the egui thread.

## Shared storage

VPM uses `vrc-get-vpm` 0.0.17 with `experimental-project-management`, including its pure Rust LiteDB implementation.
Reads combine `VRChatCreatorCompanion/settings.json`'s `userProjects` and `vcc.liteDb`; registration/removal updates
both through the library. Removing a project only removes registration. Discovery never migrates or saves that
store, and never calls upstream's `UnityProject::load`, which can repair a project's manifest.

Repositories, optional headers and caches use the same VCC environment. Repository visibility uses ALCOM's
`vrc-get/gui-config.json` `guiHiddenRepositories`; disabled packages are excluded from dependency resolution. VCC
does not understand this ALCOM UI preference. Public suggestions come from `/vpm/known` and `/vpm/public`; a failed
suggestion request leaves manual repository management available and offers Retry.

The page lists every repository version, filters pre-releases on request, installs/upgrades/downgrades/removes with
the upstream resolver and restores missing locked packages. Conflicting changes are refused before application.
Unity is opened only with the editor version recorded by the project (Unity Hub's editor lists, its default and
secondary install folders, VCC's preferred editors, then the editors VCC or ALCOM recorded in `vcc.liteDb`). A missing
editor offers Unity Hub, found through VCC's `pathToUnityHub`, then its installer's uninstall entry, then its usual
folder.

## Without Creator Companion

Nothing on the page needs VCC, as with ALCOM. The VPM folder (`%LOCALAPPDATA%/VRChatCreatorCompanion`) starts empty;
VPM creates what it needs in VCC's own format (`settings.json` and its vrc-get backup, `vcc.liteDb`, `Repos/`), so a
VCC or ALCOM installed later lists the same projects and repositories. VRChat's official and curated repositories,
never downloaded on a first start, are downloaded on the first look at the packages. The built-in templates come with
the app (above). `ORBITERS_VCC_DIR` points the app at another VPM folder and `ORBITERS_VCC_APP_DIR` at another VCC
installation (empty: none); tests use them with temporary folders (`tests/no_vcc.rs`; `cargo test -p
orbiters-projects --test no_vcc -- --ignored` downloads VRChat's repository into one). Never point them at the
person's real folders in tests.

## Package icons

Package rows show the package's own icon instead of a tinted tile once it is found (`orbiters_projects::icons`), in
this order: the logo the creator set on Orbiters, for a package of a listing Orbiters hosts (its feed at
`<website>vpm/<slug>/index.json`; the logos come from `GET /vpm/public`, kept an hour); else the icon of its author's
website (`author.url` of its newest version's manifest, or of the installed `package.json`), else of its own
(`siteUrl`). A website's icon is the largest its page declares (`link rel=icon`, `apple-touch-icon`, the web
manifest's icons; Safari's mask icon and one-colour icons skipped), else `/favicon.ico`; PNG, JPEG, WebP, GIF, ICO and
SVG are read. A GitHub address (`github.com/<owner>/…`) gives its owner's picture instead of GitHub's mark, and a
GitHub Pages site without an icon its owner's picture too. Requests run on a worker, only for the rows drawn, twelve
packages a batch and four at a time; pages are read up to 512 KB, images up to 2 MB. The answer (the icon at most
128 px, or that there is none) is kept under `Orbiters/package-icons` for a week, a day when none was found; a site
that fails later keeps the icon found before. A picture (solid on 86 % of it at least: an avatar, an app icon with
its own rounded corners) fills its tile; a mark sits on a plate, light behind a dark mark. Icons fade in over the tile.
`cargo run -p orbiters-projects --example icons -- 200 sheet.png` prints where each icon of the shared repositories'
packages came from and draws them on one sheet.

The Package repositories dialog shows each repository's icon the same way, fading in over its tile
(`icons::repository_sources`): the logo Orbiters keeps for a listing it hosts (`GET /vpm/public`), else the icon of the
website the listing is on (its host, then the domain the host belongs to: packages.vrchat.com, then vrchat.com; a
GitHub Pages listing gives its owner's picture), else the website of its packages' authors (the most common
`author.url` of its packages). A repository turned off shows it dimmed.

## Covers and world thumbnails

A project's cover is made of the pictures it has: avatars (and worlds) without one are left out of the collage (one
picture fills the cover, two sit side by side, three as one large and two small, four or more in a square with how
many more on the last), and a project without any picture shows its kind's icon. Pictures fade in over that
placeholder. In the panel's avatar list, an avatar without a picture shows its initial on a tile of its own colour.

Pictures are never taken away while their avatar or world is in the project. The window keeps each picture it showed in
`%LOCALAPPDATA%/Orbiters/project-pictures` (`thumb_cache.rs`: a JPEG per item named `<id>.<sum>.jpg`, the sum being
FNV-1a over the decoded picture; at most 600 files, 120 days), read on a worker when the window starts, so a new window
(the window process ends when it is closed) shows them at once instead of asking the tray service for each. Every
picture is still asked for once a window and when the VRChat account or session changes: one whose sum is the one shown
changes nothing, a different one fades in over the one before (`State::set_thumb`, `thumb_before`). The cards come in
the first time a window shows them only; coming back to the page, they are there already. `projects-revisit
--sequence 60` shows the page left for Settings, entered again, then every picture asked for again with one changed.

World projects' scenes are read as avatar projects' are: a PipelineManager's `wrld_…` blueprint ID is the uploaded
world (`Project.worlds`), and My World's component (its `thumbnail` and `thumbnailCamera` fields) names the world's
thumbnail, a PNG or JPEG in the project read straight from disk. Without it, the world's VRChat thumbnail comes from
VRChat when the app is signed in to VRChat (private worlds too), else from Orbiters' Map explorer
(`GET /maps/worlds/<id>`, public worlds only), kept a day under `Orbiters/world-thumbnails` (and kept longer when
neither can answer).

## Git

A project that is a Git repository has a card at the panel's end: chips for its branch, how it stands against its
upstream (commits to push and to pull, or up to date) and the changes not committed yet (a pulsing amber dot), Open
on GitHub (GitLab, or the remote's host) when it has a remote, and its last six commits on a timeline: the author's
initial on their colour on the line (the latest ringed in green), the subject, who and when, and the hash in a pill
that copies it. A press opens a commit to its whole message, its full hash and its date (the row's height springs
open, the caret turns); the rows come in one after the other the first time. `orbiters_projects::git` reads it
(`git status --porcelain=v2 --branch`, `git log`, `git remote`); a remote's address becomes its page in a browser
(`git@host:owner/repo.git`, `ssh://…` and `https://user:token@host/…` give `https://host/owner/repo`, never
credentials; local paths none).

## Mixer

While the selected project does not have `orbiters.mixer`, its panel has a Mixer card between Avatars and Packages:
Add Mixer installs it from the Orbiters repository (added or turned back on first, as for My Avatar), and its info
button opens what Mixer adds in place, with the link to https://orbiters.cc/mixer. The card grows in once the
project's packages are known and folds away once Mixer is in.

## Discovery

The cache is `%LOCALAPPDATA%/Orbiters/unity-projects.json`. Cached cards appear first; registered project metadata,
Unity Hub/Unity registry proposals and avatar results arrive progressively and are saved before the disk scan
finishes, so closing the window does not lose them. A below-normal-priority worker scans
fixed local disks at most once per hour, or on Refresh. It skips system/generated folders and reparse points, stops
at project roots, and never changes scenes or project files. A volume with no directory progress for 60 seconds is
interrupted and reported, retaining its discoveries; other volumes continue and Refresh retries it.
Avatar and world projects' scenes are read (avatars for the first, worlds and My World's thumbnail for the second). Proposals sort by the newest Windows modification time
of the root, Assets, VPM manifest or ProjectVersion file.

Avatar inspection reads text `.unity` files and PipelineManager blueprint IDs, including prefab-instance overrides.
Scene results are invalidated by file length and modification timestamp, and by referenced prefab timestamps.
Hierarchy paths are hints; Toolkit resolves the blueprint ID. Complex nested-prefab inheritance beyond the inspected
prefab's direct PipelineManager components is not fully resolved. Binary scenes and avatars without uploaded IDs
are not offered. Local Toolkit PNGs take priority; missing images are requested in batches of up to 50 from the
signed-in Orbiters API and cached separately per server under `Orbiters/avatar-thumbnails`. Credentials are never
forwarded to image hosts, and the desktop never calls VRChat's API for this page.

## Creating and duplicating

Installed VCC built-in templates (`VRCTemplates`) and user templates (`Templates`) are offered. If VCC has never been installed, modern Avatar,
World and Base templates are available offline from the bundled MIT-licensed ALCOM assets; installing the SDK still
uses VPM's official repository. With VCC installed (its uninstall entry gives the folder), the template cards show the
covers VCC shows (`WebApp/Dist/template-covers`: VRChat's templates have their own, any other one VCC's Custom cover),
loaded on a worker and fading in; without VCC the cards keep their icon. Each newly created project gets its own product GUID. Creation copies the
template into a new folder, restores packages and registers it. A custom template without ProjectVersion asks for
the exact editor version. A copy (`templates::copy_keeping`) leaves out what Unity or the IDE make again at the
project's top (Library unless asked for, Temp, Logs, obj, `.vs`, the `.csproj` and `.sln` files), UserSettings, and Git
history and `node_modules` wherever they are; an asset folder named Library or Temp deeper in is copied. It shows file
progress and can be cancelled during copying. An interrupted or failed copy stays on disk for review and is not
registered. Existing destination folders are never overwritten. A new project gets a product GUID of its own; a
template's `productName` becomes the project's name, a copy's when it was the copied project's folder name (the editor
keeps a project's PlayerPrefs under it), else it stays (`templates::retarget`). ALCOM's custom template descriptors
and unitypackage hooks are not interpreted; VCC-format user template folders are supported. A created or copied
project closes its dialog and opens in the panel once the scan lists it.

The panel's "…" menu has **Duplicate…** (`duplicate.rs`, `templates::duplicate`): the copy's name ("<name> copy" to
begin with; refused when not a valid folder name or when that folder exists), its location (the project's own folder
to begin with), Copy Library too (off: Unity imports everything again on the first opening; on: it opens at once, for
what Library takes, said with its size), about how much the copy takes against the room left on that disk (Duplicate
is refused when it does not fit), and a note that the copy's scenes keep their blueprint IDs, so uploads from it update
the same avatars and worlds unless the IDs are detached in the Pipeline Manager. The copy's progress shows in the
dialog with Stop copying; then its packages are restored, it is registered in the list VCC and ALCOM share and it opens
in the panel.

## Size on disk

The hero card's pills end with how much the project takes on disk, and what Library takes of it when there is room
(the file count and the whole sentence on hover); "Measuring…" with a spinner until the first answer. `size::measure`
walks the folder on a below-normal-priority worker (never through links or reparse points), one project at a time; the
answer is kept in `%LOCALAPPDATA%/Orbiters/project-sizes.json` with the project's stamp (the latest modification time
of its folder, Assets, Packages, ProjectSettings, Library and the VPM manifest). A project is looked up once per visit
of the page; it is walked again only when its stamp changed or its answer is a day old, the size known before showing
meanwhile. `size::free_space` gives the Duplicate dialog the room left on the destination's disk.

## Hidden projects

The "…" menu's **Hide this project** (Show this project once hidden) keeps the project's folder in
`%LOCALAPPDATA%/Orbiters/hidden-projects.json` (`hidden`, by `discovery::key`; saved on a worker, a save started
earlier never landing over a later one). Hidden projects leave the cards and Found on this computer, the counts in the
header and the sections' pills, and the choice of the project the docked panel shows first. A chip at the right of
"Your projects", **Hidden · n**, shows them again, dimmed and marked Hidden (a found one says so in its line), until
pressed again. Hiding the project shown moves the panel on (a sheet slides away). VCC and ALCOM keep listing them;
nothing in the projects changes.

## Right-click menus

A right press on a project card or on a project found on the computer opens its menu (`menus.rs`, through
`orbiters_ui::popup::context_menu`): Open in Unity (Add to my projects for a found one), Open the folder, Copy the
folder's path, Duplicate…, Hide or Show this project and Remove from the list, the actions the panel's "…" menu shares.
A right press on an avatar's or a world's row in the panel: Open on VRChat, Copy its ID, Change the picture on VRChat…
(the signed-in VRChat account's own, as its thumbnail's hover), Add N4N0 and Install My Avatar only (an avatar of a
listed project) and Open in Blender for each linked .blend. Screenshots: `projects-card-menu`, `projects-avatar-menu`.

## Avatars and worlds on VRChat

Each avatar row has Open on VRChat (`https://vrchat.com/home/avatar/<avtr_id>`, `controller::open_url`) at its top
right. A world project (its SDK, or worlds and no avatars in its scenes) has a **Worlds** card in place of the Avatars
card (`world_card.rs`): each world with its picture (16:9, `art::avatar`), its name (VRChat's from its log when this
computer visited it, else the scene object's unless it is the SDK's `VRCWorld`, else the project's), its scene, what
this member already has on Orbiters' Map explorer (`ctl.maps()`, asked for once a visit: a 3D preview, how many
pictures, or Not on Orbiters yet; a shimmer until Orbiters answers) and Open on VRChat
(`https://vrchat.com/home/world/<wrld_id>`). Under it a well says that what is added shows on orbiters.cc's Maps page
and in the VRChat world Orbiters 3D pictures viewer (it reads the same server's `/maps/vrchat/*` routes), with:

- **Share my 3D capture** (a dropdown, Share a 3D capture, when several): the sessions of the library recorded in that
  world and reconstructed (`crate::worlds::load`, read once per visit and library change), through
  `ctl.upload_preview`; its splats become the world's 3D preview on Orbiters, in place of this member's earlier one.
  Without one the button says on hover how to make one.
- **VRChat pictures**: a multi-file dialog (PNG, JPEG, WebP) opening in the folder of the newest picture taken in that
  world, else VRChat's pictures folder (`visited::screenshot_dirs`); **From this computer**: the same, in Pictures.
  Both go through `ctl.upload_pictures`. Dialogs run on a worker.
- Signed out of Orbiters: Sign in to Orbiters to add (Finish in your browser while it waits).

The world's latest upload shows under them, from the snapshot's uploads as on the Visited worlds page: Sending to
Orbiters… (handed to the tray service, until it lists it), Preparing the 3D capture…, Sharing … · n % with a bar, then
how many pictures were added (and how many were there already), the 3D capture now the world's preview, or why not.

## Blender files

Avatars' and worlds' rows list the .blend files Orbiters' Blender tools linked them to, each with the tool, when it was
saved, its path on hover and **Open in Blender** (`blend.rs`, `orbiters_projects::blender`), which shows Blender's
logo (`icons::BLENDER`, `orbiters_ui::blender_logo`: the logo's SVG outlines in one colour, as Blender's logo guidelines
allow for linking to Blender), as each file's tile does. The links are read from
the project when it is opened and after each scan (small files only):

- Mixer: `Assets/Orbiters/Mixer/Link/<scene GUID>.asset` (`blendPath`, written by "Open scene in Blender") and the
  .blend files in `Assets/Orbiters/Mixer/Blender/<scene GUID>/blend~/`. The scene's GUID is matched with the `.meta` of
  the avatars' and worlds' scenes; a Mixer link goes to every avatar or world of its scene.
- MCB before 1.12.11 (it now goes through Mixer): `Library/MCB/BlenderSync/<session>/session.json`
  (`customBaseGlobalId`'s scene GUID, `blenderProjectAbsolutePath`, else `blenderProjectUnityPath`) and, for a .blend
  picked by hand, the session's `blender_heartbeat.json` (`blendFile`); and the .blend files MCB made,
  `Assets/MCB/blenderProjects/<avatar root>_<12 hex>[_n]/(blend~/)<avatar root>.blend`. A link that names an avatar
  root goes to the avatar of that name (MCB's folder-safe spelling too); one that does not, to the avatars of its scene.

Each .blend is listed once, newest first, only if it exists; `.blend1` backups are left out. Open in Blender looks for
Blender where .blend files open (`HKEY_CLASSES_ROOT\.blend`'s ProgID `shell\open\command`), Steam's (each library of
`libraryfolders.vdf`, `steamapps/common/Blender`) and `Program Files\Blender Foundation\Blender *`, each's version
being the newest version folder beside its program. It starts the newest that has Mixer's or MCB's add-on in Blender's
user settings (`%APPDATA%/Blender Foundation/Blender/<version>/scripts/addons` or `extensions/*`), else the newest,
else the associated one (the association alone can be an old Blender that would not open a newer file), with the
file and its folder as working directory; without any, the shell opens the file.

## Adding N4N0

Add My Avatar ensures the Orbiters repository and installs `orbiters.toolkit` and `orbiters.myavatar`. Every "Add
N4N0" control (the panel's buttons, Capture's N4N0 card, the avatar picker and its setup dialog) shows the N4N0 mark
(`icons::N4N0_MARK`), not N4N0's face. Add N4N0 also
writes the schema-1 request specified in `contracts/Docs/project-setup.md`, through `<uuid>.json.tmp` and rename.
Toolkit owns confirmation, gallery installation, scene saving and the SDK panel. The desktop checks UnityLockfile,
offers Open project, polls the result while the setup dialog is open, displays it and then consumes the result.
After success it tells the person to upload from the SDK and return. Capture's ready message uses the worn avatar's
existing manifest and N4N0 `prop_sync` status.

Development's N4N0 gallery ID is 30. Production is deliberately unset in
`orbiters_core::account::N4N0_PRODUCTION_GALLERY_ASSET_ID`; fill it after publication. Production refuses N4N0 setup
until then, while Add My Avatar remains available. The request contract has no server/environment field despite
server-scoped gallery IDs; the desktop and My Avatar must use the same environment.

## Package updates

After each look at the selected project's packages (opening its panel, a change, Refresh) a worker checks its updates
(`orbiters_projects::updates::check`, against the repositories as last downloaded); the repositories themselves are
downloaded again once a window and every half hour while a project is shown, quietly (a failure keeps the cached
view). Each installed package's newer versions are tried newest first with the resolver the version lists install
with (`add_package_request`, as `vpm::change` does): the project's Unity version, the other packages' dependency
ranges, releases only unless Pre-release versions is on (both answers are kept, the switch picks one). An update that
would also move a VRChat SDK package (`com.vrchat.*`) is not offered for another package; the SDK's own rows offer
theirs. The row's second line then says **Update to x.y.z** (its hover names what else it changes) or, for a newer
version needing other changes, **Update to x.y.z…** in amber, which opens the conflict dialog below. **Update all · n**
in the card's heading opens the plan above the rows (every package from → to, what is left out and why: the SDK,
updates that do not go together) and applies it at once. Updates go through `panel::package_action`, the version
lists' path. A manifest still holding the old "3.2-3.8" range spelling (which `UnityProject::load` rewrites) is not
checked automatically.

## Packages missing from the repositories

When a change fails with the resolver's "Following dependencies are not found: …", the panel's error banner becomes a
card naming those packages with **Add their repositories** (`missing_card.rs`, `orbiters_projects::missing`). It looks
them up in Orbiters' Known VPM catalog (`GET /avatar-assets/known-dependencies`, public; repositories named from
`GET /vpm/known`), then the person's repositories turned off on this computer, then the listings creators publish on
Orbiters (`GET /vpm/public`, each feed read, at most 40). A pinned version (`=1.1.5`) is found only in a repository
listing it; a Known VPM repository naming the package without that version (the catalog keeps 40 versions in its
supported range) is read itself first. The card then lists each repository to add (or turn back on) with the
packages and versions it brings, the packages listed only at other versions, and those no repository provides;
**Add n repositories and retry** adds them (`missing::add`) and runs the failed change again (the last change, else
Restore). The card goes once it succeeds; what still fails comes back in a new card.

## Conflicts

An install that conflicts (an installed package's range does not accept a version it installs) is refused as before,
with `coherent::Conflicted` carrying the plan that solves it (`orbiters_projects::coherent`): the asked versions, then
each package whose range broke moved to its newest version accepting what the plan installs (Gesture Manager,
Av3Manager and the like along with the SDK, the SDK's base brought by its avatars package), until nothing conflicts.
The **Update to coherent versions** dialog (`conflict.rs`) shows what conflicts (who accepts which range), every
change (from → to, added, removed) with why, and **Update n packages together**, which applies `vpm::Change::Plan`:
removals, then the exact versions, the manifest and its packages put back on any failure. A package with no fitting
version blocks the plan, named with the range it holds; **Remove it too** solves again without it when nothing else
needs it. Cancel changes nothing.

## A project open in Unity

`orbiters_projects::editor` reads the Unity editors open from their command lines (`-projectPath`; import workers and
batch mode excluded; an unreadable command line is matched by the window title while the project's lock file is
held), every second and a half on a worker while the page is shown. A project open in Unity has, in place of Open in
Unity (hero card, compact bar, right-click menus): **Show** (its window to the front, restored when minimised, or the
dialog it waits on), **Save & Exit** (`WM_CLOSE` to its main window, as its close button: Unity asks whether to save
modified scenes, that question brought to the front and the button saying Answer in Unity; a toast when it quit, or
when it stayed open after the question) and **Kill** (after Kill, losing unsaved changes: that editor and its own
import workers ended, other editors untouched, nothing in the project deleted). The Toolkit has no command channel
with the desktop, so saving is Unity's own question rather than a silent save.

## Verification

Set `CARGO_TARGET_DIR=C:/Users/Enzo/.orbiters-target` and `CARGO_INCREMENTAL=0` for every build. Focused checks:

```powershell
cargo test -p orbiters-projects
cargo check -p orbiters-desktop
cargo run -p orbiters-projects --example discover
orbiters-desktop --screenshot <folder> --sessions <empty folder> --only projects-gallery
```

Other deterministic offscreen states: `projects-detail`, `projects-packages`, `projects-repos`, `projects-new`,
`projects-avatars`, `projects-choice`, `projects-setup`, `projects-ready`, `projects-mixer` (its info open),
`projects-mixer-open-mid`, `projects-mixer-fold-mid`, `projects-mixer-adding`, `projects-commits` (the Git card),
`projects-commits-open` (a commit opened), `projects-commits-mid` (its rows coming in), `projects-commits-narrow`
(with `--width 1040`), `projects-world` (a world project with its thumbnail and its Worlds card before Orbiters
answered), the panel's additions in `screenshot/projects_panel.rs`: `projects-size` and `-size-measuring`,
`-avatars-links` (Open on VRChat and Blender files), `-worlds` (what Orbiters has, three captures, a Blender file),
`-worlds-new`, `-worlds-sharing`, `-worlds-done`, `-worlds-failed`, `-worlds-signed-out`, `-worlds-capture-menu`
(the captures' list open), `-duplicate`, `-duplicate-taken`, `-duplicate-library`, `-duplicate-copying`, `-hidden`,
`-hidden-shown`, `-hidden-press` (the chip pressed), `-header-blur` (the cards under the header's blur), `-updates`,
`-updates-plan`, `-missing`, `-missing-looking`, `-missing-found`, `-missing-adding`, `-missing-done`, `-conflict`,
`-conflict-blocked`, `-unity-open`, `-unity-kill`, `-unity-closing`, `-unity-killing`, `-unity-bar` and `-unity-menu`
(`package_fixtures.rs`; `--only a,b,c` renders several in one run); the
Worlds card shows whole with `--height 1300`. `projects-repos-crossfade` (repository
icons fading in), `projects-repos-live` (review only: the repositories' real icons, cache then network), `projects-new-novcc` and
`projects-repos-novcc` (a computer without VCC), `projects-new-loading` (covers fading in) and `projects-new-covers`
(VCC's real covers when it is installed). Package icons in screenshots are stand-ins (pictures, light and dark marks,
ReFit without one); `projects-crossfade` shows them fading in. `projects-icons-live` (review only, not deterministic)
gives the stand-in packages their real listings and websites and looks their icons up as the page does: the disk
cache, then the network. These stand-ins never scan or mutate real
projects, make HTTP requests or send setup requests. Unit fixtures cover version parsing, VRChat manifests, registry
binary paths, scene/prefab avatars, skip rules, copying, setup JSON/results and a temporary VCC registration store.

The loopback repository integration test exercises real ZIP downloads, dependency installation, upgrade, downgrade,
missing-package restore, conflict refusal, repository headers/visibility/removal and creation/duplicate resolution,
entirely in disposable folders. `tests/package_updates.rs` does the same for updates (releases and pre-releases,
dependency ranges, the SDK held back, Update all applied), missing packages' repositories found, added and resolved
again, and the N4N0 sample avatar's SDK conflict (com.vrchat.avatars/base 3.7.5 with Gesture Manager, Av3Manager and
Fuck OSC Config pinning it) solved into one plan, blocked until removed, and rolled back on failure. Manual acceptance still requires public-repository downloads, VCC/ALCOM cross-app
synchronization, Toolkit confirmation and a VRChat SDK upload. Those writes must use disposable projects.
