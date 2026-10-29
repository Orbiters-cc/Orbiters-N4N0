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
Unity is opened only with the editor version recorded by the project. A missing editor offers Unity Hub.

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
uses VPM's official repository. Each newly created project gets its own product GUID. Creation copies the
template into a new folder, restores packages and registers it. A custom template without ProjectVersion asks for
the exact editor version. Duplication excludes Library, Temp, Logs, obj, UserSettings, node_modules and Git history;
it shows file progress and can be cancelled during copying. An interrupted or failed copy stays on disk for review
and is not registered. Existing destination folders are never overwritten. ALCOM's custom template descriptors and
unitypackage hooks are not interpreted; VCC-format user template folders are supported.

## Adding N4N0

Add My Avatar ensures the Orbiters repository and installs `orbiters.toolkit` and `orbiters.myavatar`. Add N4N0 also
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
`projects-avatars`, `projects-choice`, `projects-setup`, `projects-ready`. These stand-ins never scan or mutate real
projects, make HTTP requests or send setup requests. Unit fixtures cover version parsing, VRChat manifests, registry
binary paths, scene/prefab avatars, skip rules, copying, setup JSON/results and a temporary VCC registration store.

The loopback repository integration test exercises real ZIP downloads, dependency installation, upgrade, downgrade,
missing-package restore, conflict refusal, repository headers/visibility/removal and creation/duplicate resolution,
entirely in disposable folders. Manual acceptance still requires public-repository downloads, VCC/ALCOM cross-app
synchronization, Toolkit confirmation and a VRChat SDK upload. Those writes must use disposable projects.
