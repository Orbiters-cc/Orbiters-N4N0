# 3D preview of a Unity project (29 October 2026)

Enzo asked for a quick 3D preview of a Unity project in Orbiters - N4N0 before opening it, loading in at most about
3 seconds. The Unity projects page now shows the selected project's last saved scene in 3D in its panel's hero card,
in place of the avatar pictures, and grows it into a window over the page on a press. The file format is in contracts'
Docs/project-preview.md. Since 3 November 2026 a project Toolkit never wrote one for gets one built from its own files,
for each of its scenes (below).

## Built from the project's files (3 November 2026)

Enzo asked for previews of projects that were never opened, or never with Orbiters Toolkit, and for every scene of a
project, not only the one saved last. Orbiters - N4N0 now builds a scene's preview itself from the project's files
(`orbiters_projects::scene`), in the same format Toolkit writes, so the viewer draws both alike:

- **What it reads.** The scene's YAML with every prefab instance resolved as Unity resolves it (nested prefabs,
  variants, overrides, removed components and GameObjects), models through ufbx (FBX binary and ASCII, OBJ) as Unity
  imports them (X mirrored, triangles reversed, file units to metres, Z-up files turned upright, Unity's object IDs:
  xxHash64 of `Type:<class>->//RootNode/root/<path>0`), skinned meshes posed by their bones after their blend shapes,
  Unity's built-in meshes and mesh `.asset` files saved as text, materials (`.mat`) by shader family with Toolkit's
  rules (lilToon, Poiyomi, MToon, Unity Toon Shader, Standard), their main and emission textures (PNG, JPEG, TGA, BMP,
  TIFF, GIF, PSD and PSB's merged picture) shrunk to Toolkit's budget, VRChat avatar and world descriptors, the
  scene's directional lights and gradient or flat ambient light. GUIDs come from Unity's own database
  (`Library/SourceAssetDB`, read as LMDB without LMDB) when the project was opened once, else from the `.meta` files
  (read on 8 threads, the kinds a preview needs first, kept in the cache by modification time): on Enzo's hard disk a
  cold `.meta` costs about 12 ms, 87 s for a 7,000-asset project, against 0.3 s for the database.
- **Never into the project.** Previews and what they were built from go to `%LOCALAPPDATA%/Orbiters/project-previews/
  <project>/` (`<scene GUID>.preview`, `<scene GUID>.json`: every file read with its size and time, or why the build
  failed), at most 1.5 GB, the least recently used first out.
- **Which preview a scene gets.** Toolkit's when it is at least as new as the scene file (2 s of slack for copied file
  times): Unity drew it, it is exact. Else the one built here while none of the files it read changed (sizes and
  modification times, the project's package manifests too, at most 14 days); a changed one stays on screen while it is
  built again. A failed build is kept with its reason until a file it read changes.
- **Scenes.** All of a project's scenes under `Assets` (and those in its build settings): the one Unity had open last
  first (`Library/LastSceneManagerSetup.txt`), then the build settings' in order, then the others from the most
  recently saved. The card opens on the first (another that has a preview when the first cannot be built). The window
  lists them as chips, as many as fit between the facts and the buttons (the shown one always), the others under
  "All scenes · N"; a chip says when its scene is building (a spinner) or has no preview (a warning, the reason on
  hover). Picking one not built yet builds it first; the scene before stays, dimmed under "Building <scene> from the
  project's files…", or "No preview of <scene>: <why>".
- **In the background.** One worker (`pages/projects/builds.rs`, below normal priority, its picture threads too) builds
  the scene the panel or the window asks for before anything else, then draws the gallery cards' stills. A build
  stops with its reason after 120 s, at a model over 600 MB or 6 million triangles, ufbx past 2 GB of memory, a scene
  or prefab over 256 MB, a texture over 256 MB or 8192 pixels on a side.
- **The gallery's cards.** A project whose card has no avatar picture gets a still of its first scene, drawn by the
  same renderer as the 3D view on the worker (640×360, read back, kept as `cover.jpg` beside the preview): one kept
  from before shows at once, a new one only when its preview changed. Stills are asked for in the gallery's order
  whatever is on screen, two seconds after the window opens (the avatar pictures kept from before show first), and only
  change a card's picture, never its place or size. Avatar pictures stay first: they are what the person chose.
- **On the card without a preview**: "Building 3D preview…" with a spinner while it builds, "No scene to preview" for
  a project without one, "No preview: <why>" (the full reason and what Toolkit would do on hover), beside "Add Toolkit
  for 3D" or "Update Toolkit for 3D" when the project's Toolkit can be added or updated.

## How it looks and behaves

- **In the card.** As soon as the project is selected its preview loads (usually in tens of milliseconds). The avatar
  pictures give way to a dark studio, a green ring runs out over the floor and the scene rises out of it behind a
  glowing green edge (1.25 s, eased in and out) while the camera swings round to the front (avatars) or settles on
  the Scene view's camera (worlds). A drag turns it with inertia (a flick keeps it spinning a moment, then it eases to
  a stop); the wheel keeps scrolling the panel. A press without a drag, or the corner button, opens the window.
- **The window** grows out of the card with the shared-geometry morph of the picture windows (`viewer::Morph`,
  `Spring::EXPAND`) and shrinks back into it with Escape, the ×, or a press beside it. There the view also zooms
  (wheel, pinch, smoothed in log space) and pans (right or middle drag); a double click or the round button frames it
  again the short way round. A title chip names the scene, the project and when it was saved; chips say how many
  avatars, triangles and materials, with an ⓘ when meshes were left out for the budget or could not be read; Open in
  Unity sits bottom right; a project with several previews (one per scene) lists them as chips to switch between. A
  hint on how to move around fades once the view was turned or after 5 s.
- **Lighting** is the scene's own when Toolkit recorded it (contracts' Docs/project-preview.md, `lighting`; Enzo
  chose it as the default on 2 November 2026): its main directional light as the key with its colour (intensity
  included) and shadow strength, a second directional light beside it, its ambient light facing up, round the horizon
  and down, its reflections from that ambient light at its reflection intensity, nothing of the studio's. A switch at
  the top of the window, beside the close button, shows "Scene light" or "Studio light" instead (remembered for every
  preview in `project-preview.json` in the configuration folder; the scene's light by default); it is there only
  when the file recorded the scene's light, and the view changes as soon as it is pressed (the shadow map is drawn
  again from the new key). A preview without it, from Toolkit 0.4.14 or older, keeps the studio light.
- **The studio light** is the desktop's own, the same for every project and as bright as Unity's default scene (2
  November 2026, after Enzo found his avatar glossy and redder than in Unity): Unity's default directional light (colour
  1, 0.956, 0.839, intensity 1) as the key from the front left of the avatar, about 50° up as in a new scene, with a
  soft shadow (2048² shadow map, 12-tap Poisson PCF); the default procedural sky's ambient light (blue-grey above, dark
  below) and its reflections; a faint cool fill and rim on lit materials so the side away from the light keeps its
  shape. No tone mapping below 0.8 (Unity's Scene view has none), Khronos PBR Neutral's shoulder above; 4×
  multisampling. Before, Neutral's toe pushed dark browns towards orange and a fixed Blinn highlight made everything
  look like plastic.
- **Materials** look as their shader shows them (contracts' Docs/project-preview.md, Materials, schema 2): `lit`
  is Unity's Standard BRDF (Lambert, GGX with Smith-joint visibility and Unity's π scaling, Fresnel-weighted sky
  reflection), with the material's metallic and smoothness (and its metallic-smoothness picture); `toon` two bands on
  the half-Lambert term with the shader's shade colour, border and softness and the light capped at white (Poiyomi,
  lilToon, MToon, Unity Toon Shader); `flat` (Poiyomi's default Flat lighting, lilToon without shadows) evenly lit;
  `unlit`. Normal maps bend the normal in a frame made from the UVs' screen derivatives (no tangents in the file),
  emission adds its colour. A preview Toolkit 0.4.12 or older wrote (schema 1) has no look: its materials draw `lit`
  with smoothness 0.2 and metallic 0, nearly matte, which is what most avatars are. Avatars
  stand on a floor of our own: a pool of light under them, the shadow, a contact shadow and the scene mirrored faintly
  in it, fading with height and towards the floor's edge into the background. Worlds have their own floors and get
  none.
- **Live.** The shown project's `UserSettings` folder is watched (Windows change notifications,
  `orbiters_projects::preview::watch`), so a preview Toolkit writes shows as soon as it is on disk (it rises again
  where the camera is); the previews are also listed again every 15 s. Switching back to the project shown just
  before is immediate.
- **Without a preview** the avatar pictures stay and a chip on the cover says why, with the full explanation on hover:
  "3D after a save" (a Toolkit that writes previews is in the project: save the scene), "Update Toolkit for 3D" (its
  Toolkit is older than 0.4.10, the first that writes them: a press updates it to the newest release the repositories
  offer; an open editor then writes the open scene's preview once it reloaded), "Add Toolkit for 3D" (no Toolkit: a
  press adds it), or "3D preview unreadable" with the reason. The two offers are green and show a spinner while the
  package changes. A spinner shows while a preview loads.

## Why Toolkit's snapshot first

Nothing outside Unity can draw a project's scene cheaply: scenes reference prefabs, FBX models, materials of custom
shaders (Poiyomi, lilToon, locked variants) and textures whose usable forms live in `Library`; posing an avatar needs
its skinning, blendshape weights and constraints. The desktop's own builder (above) is a second, partial Unity
importer: seconds to a minute the first time, and never quite right where Unity's own processing matters (shaders'
real looks, constraints, tools that build the avatar at upload such as VRCFury or Modular Avatar, meshes saved in
Unity's binary format). Inside Unity all of it is a few calls away, so Toolkit keeps a small
ready-to-draw copy (`UserSettings/Orbiters/Previews/<scene GUID>.preview`): posed meshes, base-colour pictures (JPEG,
PNG where alpha matters) and where everything stands. The desktop reads the file in one go and uploads its vertex and
index blobs as they are; it converts nothing. Its preview is used whenever it is as new as the scene; the builder
fills in the other scenes and the projects without Toolkit.

Toolkit writes one a few seconds after a scene is saved (the last of several saves), after an avatar upload, and once
after the editor loads when the open scene has none or an older one. It reads the scene in steps on
`EditorApplication.update` (one mesh copied, one skinned mesh baked or one picture drawn through the GPU per step,
the bake and the copy of a skinned mesh in separate steps), reads the pictures back asynchronously and leaves the
encoding and writing to a worker thread. It changes nothing in the scene, its assets or the Undo history. Four
previews per project are kept (one per scene).

## Measured

Unity side, MCB Test, MasculineCanine's scene (`Canine.unity`: the avatar with N4N0, an Ultirex model beside it; 6
renderers, 119,704 triangles stored, 7 pictures, 3.8 MB file), Unity 2022.3.22 in the background:

| Capture | Main thread in all | Longest step | Worker (encode, write) | Start to file |
| --- | --- | --- | --- | --- |
| First after a domain reload | 75.7 ms | 27.4 ms | 81 ms | 443 ms |
| Warm (three runs) | 10.7–14.2 ms | 4.1–5.9 ms | 55–113 ms | 85–155 ms |

Before the bake and the copy of a skinned mesh were split, the body (89k vertices, 497 blendshapes) made one 8.5 ms
step. A save in Unity was followed by the capture about ten seconds later in the background editor (3 s of debounce,
then its update loop's pace).

Desktop side (`project preview …` on stderr; read, pictures decoded with mipmaps on up to 8 cores, upload, pipelines
compiled on another thread at the same time):

| Preview | Build | First in a process | Warm |
| --- | --- | --- | --- |
| MasculineCanine (3.8 MB) | release | 259 ms (pipelines 218 ms) | 22–29 ms |
| MasculineCanine | debug | 217 ms | 96–165 ms |
| Synthetic world at the budget (33 MB: 2M triangles, 200 meshes, 2,000 instances, 64 pictures) | release | 101–230 ms | |

All far under the 3 s Enzo allowed; the reveal itself then takes 1.25 s and the card shows the studio from the first
frame. The file was in the system's cache in these runs; a 3.8 MB read from a cold SSD adds a few milliseconds, from
a hard disk about 50 ms. The first preview of a session compiles the pipelines (about 0.2 s with the driver's shader
cache, longer the very first time after a driver update), in parallel with reading the file.

## Code

| Where | What |
| --- | --- |
| `orbiters_projects::preview` | the format: header types, `read`/`parse` (every blob checked to lie in the file and fit its use), `list` (headers only, newest first), `Writer`, octahedral normals and half floats; tests |
| `pages/projects/preview_gpu.rs` | loading (parallel decode and mipmaps, pipelines on another thread) and drawing: WGSL, shadow pass, mirrored pass, ground, background, MSAA resolved into a texture egui samples (`register_native_texture`), drawn again only when the view changes |
| `pages/projects/preview.rs` | listing (scenes and their previews) and loading jobs, the camera (orbit, goal, inertia, swing-in, reset), the card's cover and its notes, the window and its scene chips, the stand-in scene and the real-project fixtures for screenshots |
| `orbiters_projects::scene` | previews built from a project's files: `scenes`, `list`, `build_scene`, the cache and its freshness (`mod.rs`); `yaml`, `assets` (GUIDs, Unity's LMDB database), `prefab`, `model` (ufbx), `material`, `texture` (PSD/PSB too), `mesh_asset`, `primitives`, `build` (posing, budget, writing); `examples/scene_preview.rs` builds real projects' scenes and prints the table below |
| `pages/projects/builds.rs` | the background worker: builds first, then the cards' stills (drawn with `preview_gpu::Scene::still`, read back, kept as `cover.jpg`) |
| Toolkit `Editor/Preview/ProjectPreview.cs`, `PreviewFile.cs` | the capture and the writer; `ProjectPreview.CaptureNow`, `LastResult` for diagnostics |
| Toolkit `Editor/Preview/PreviewLook.cs` | each material's look by its shader family: shading, metallic, smoothness, normal, surface and emission pictures |

The texture egui paints is the resolved picture, so the card's squircle corners, the crossfade over the avatar
pictures and the window's morph come from the usual painters; its size grows with the window and shrinks once the
window is back in the card.

## Screenshots

```powershell
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-preview
$env:ORBITERS_PREVIEW_FILE = "<a .preview Toolkit wrote>"   # instead of the stand-in scene
$env:ORBITERS_SCREENSHOT_NO_PICTURES = "1"   # skip this computer's VRChat pictures index (hours on a large archive)
```

Scenarios: `projects-preview` (settled), `-reveal` (rising, 0.5 s in), `-swing` (1 s in), `-turned` (dragged),
`-open-mid` (the window six frames into its morph), `-open`, `-close` (the window on the upper half from close by, to
judge materials), `-back` (from behind, the side away from the light), `-scene-light` (the window in the scene's
recorded light) and `-studio-light` (the same after a press on the switch's Studio light), `-scenes` (two scenes to
switch between), `-sheet` (with `--width 1040`), `-none`, `-no-toolkit` and `-old-toolkit` (a scene with nothing to
draw: "No preview: …", with the Toolkit offers), `-building` (a build under way), `-no-scene`. With
`ORBITERS_PREVIEW_PROJECT` set to a Unity project's folder (read only; set `ORBITERS_PREVIEW_CACHE` to keep its
previews out of the app's cache): `-native` (its last-open scene built on the frame), `-native-building`,
`-native-open` (the window and its scene chips), `-native-switch` (the second scene picked, building),
`-native-switched` (the same once built), `-native-all` (the list of every scene). `projects-stills` shows the gallery
with the projects `ORBITERS_PREVIEW_PROJECTS` names (separated by `|`), without avatar pictures, their stills drawn on
the frame. The stand-in is
N4N0's black sphere with white strokes over a plinth, a green ring and two cubes, as an avatar scene, one material look
each: a glossy sphere, a matte plinth, a glowing unlit ring, a toon cube and a metal one.

## Not done or untested

- A real world project's capture (only the synthetic world was timed and drawn); terrains and lightmaps are not in the
  format. Of a scene's lights only directional ones are used (two of them, the main one's shadow), not point and spot
  lights, light probes, a skybox's picture in reflections (its ambient light's three colours stand in), fog or
  post-processing; nor occlusion and detail maps, toon ramps' pictures, matcaps, rim lights and outlines of toon
  shaders, specular-setup colours, or metallic and smoothness from two different textures (the smoothness one is kept).
- The first capture after a domain reload makes one step of about 27 ms (JIT and first use); later ones stay under
  about 6 ms.
- Non-readable meshes (FBX without Read/Write) were checked readable in Edit Mode without errors (FashionSuit's), and
  a non-readable skinned body bakes; a project in Play Mode or compiling waits.
- The preview needs a Toolkit release with `ProjectPreview` and a desktop release; until then projects show the note.
- Built from the files: meshes saved in Unity's binary serialization (MCB's generated body, 160 MB) and `.blend`
  models are left out (counted, the reason in the cache's record); shaders' looks are Toolkit's rules on the saved
  properties, not the shaders themselves (a missing shader draws its texture rather than Unity's magenta); animators,
  constraints and what VRCFury or Modular Avatar do at upload are not applied (the scene as saved); a skinned mesh's
  bones are the renderer's; normal, metallic-smoothness and occlusion maps are not read; the Scene view's camera is
  not known (worlds open over a corner of their bounds).
