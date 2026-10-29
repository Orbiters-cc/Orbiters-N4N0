# Unity projects and avatar setup

The **Unity projects** page, immediately after Capture, manages the VCC/ALCOM project list and VPM packages without
requiring Creator Companion to run. **Capture → Add to my avatar** uses the same project and avatar inventory.

`crates/orbiters-projects` owns discovery, Unity text-scene inspection, VPM operations, templates, thumbnails and the
setup-request contract. The desktop's `pages/projects` holds the window's background jobs and their snapshots. Disk
walks, file dialogs, Git, package resolution, copies, thumbnail decoding and HTTP requests run off the egui thread.

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
interrupted and reported, retaining its discoveries; other volumes continue and Refresh retries it. Proposals sort by the newest Windows modification time
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
the exact editor version. Duplication excludes Library, Temp, Logs, obj, UserSettings, node_modules and Git history;
it shows file progress and can be cancelled during copying. An interrupted or failed copy stays on disk for review
and is not registered. Existing destination folders are never overwritten. ALCOM's custom template descriptors and
unitypackage hooks are not interpreted; VCC-format user template folders are supported.

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
`projects-mixer-open-mid`, `projects-mixer-fold-mid`, `projects-mixer-adding`, `projects-new-novcc` and
`projects-repos-novcc` (a computer without VCC), `projects-new-loading` (covers fading in) and `projects-new-covers`
(VCC's real covers when it is installed). Package icons in screenshots are stand-ins (pictures, light and dark marks,
ReFit without one); `projects-crossfade` shows them fading in. `projects-icons-live` (review only, not deterministic)
gives the stand-in packages their real listings and websites and looks their icons up as the page does: the disk
cache, then the network. These stand-ins never scan or mutate real
projects, make HTTP requests or send setup requests. Unit fixtures cover version parsing, VRChat manifests, registry
binary paths, scene/prefab avatars, skip rules, copying, setup JSON/results and a temporary VCC registration store.

The loopback repository integration test exercises real ZIP downloads, dependency installation, upgrade, downgrade,
missing-package restore, conflict refusal, repository headers/visibility/removal and creation/duplicate resolution,
entirely in disposable folders. Manual acceptance still requires public-repository downloads, VCC/ALCOM cross-app
synchronization, Toolkit confirmation and a VRChat SDK upload. Those writes must use disposable projects.
