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
  scene's directional lights and gradient or flat ambient light; for worlds, see "Worlds" below. GUIDs come from Unity's own database
  (`Library/SourceAssetDB`, read as LMDB without LMDB) when the project was opened once, else from the `.meta` files
  (read on 8 threads, the kinds a preview needs first, kept in the cache by modification time): on Enzo's hard disk a
  cold `.meta` costs about 12 ms, 87 s for a 7,000-asset project, against 0.3 s for the database.
- **Never into the project.** Previews and what they were built from go to the app's cache folder,
  `%LOCALAPPDATA%/Orbiters/Desktop/cache/project-previews/<project>/` (`<scene GUID>.preview`, `<scene GUID>.json`:
  every file read with its size and time, or why the build failed; `guids.txt`, the cards' `cover.jpg`), at most
  1.5 GB with everything in it counted, the previews shown least recently first out (their record's time, touched
  when shown: Windows keeps no last-access times on large disks), a project left without one going whole. A
  screenshot run builds into a cache of its own (`screenshot.rs`, `sandbox_user_data`).
- **Bounded on odd files.** Reviewed 3 November 2026 against damaged and hostile files: a flow value nested deeper
  than 64 is passed over (it once looped, filling the memory), a mesh asset's stream, offset and counts are checked
  against Unity's ranges and its buffers, a binary file makes at most 4 million values and never repeats records of
  no size, Unity's database read gives up on a tree whose branches point back up, a PSD header cannot ask for more
  than its file could hold, nested prefabs copy at most a million objects, and a source that failed is not read again
  for each instance. A build that panics anyway is caught on the worker (the scene says so until its files change).
- **Retried, stopped, never waiting.** A build that ran out of time (the first look at a project never opened, on a
  hard disk) is tried again after 30 minutes, not kept as a failure for two weeks; a build the panel or the window
  asks for stops the card still or other build running (put back in its queue); the app closing stops the build
  running; a scene saved during its build is built again.
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

## Worlds (4 November 2026)

Enzo opened his Avatorbit world and saw six tiny avatars and two campfires in a dark void, 81k triangles of a scene that
is a desert, a sea and a large building. Reading that scene to find out why, reason by reason (`Report::dropped`, the
`scene_preview` example prints it):

| What Avatorbit's `VRCDefaultWorldScene` lost | Renderers | Cause | Now |
| --- | ---: | --- | --- |
| **Its sand, water, rocks, palm trees, the building: all of `avatorbit.blend`, and `branch pen.blend`** | 307 in all | The whole world is one Blender file, which only Unity's import can read (it runs Blender) | Read from what that import left in `Library` |
| Its sun | 1 light | A light *inside* the `.blend`, which is also `RenderSettings.m_Sun`; the "Directional Light" object is off | Lights of imported models are read |
| Its baked light | 188 lit by 7 lightmaps | Not in the format | Lightmaps, from Bakery's storage or `LightingData` |
| Its sky and fog | | Not in the format | Sky picture, fog |
| Renderers off, inactive, `EditorOnly`, without a mesh | 58 of 478 | As Unity draws them | Still not drawn, as in Unity |

There is no Terrain in that scene at all: the sand that looks like one is a mesh in the `.blend`. Terrains are read now
too (below), and were tested on the project's real `SandTerrain.asset` (BruteForce's demo scene).

- **What Unity's import left in `Library`** (`scene::artifact`). A project opened once keeps each imported asset as a
  binary file of its own (`Library/Artifacts/<first two digits>/<hash>`): the GameObjects, transforms, renderers,
  meshes, materials and lights the import made, under the very IDs prefabs and scenes refer to them by. Which file comes
  from `Library/ArtifactDB`, an LMDB file of hundreds of megabytes read a page at a time without LMDB (`CurrentRevisions`
  once, a few pages per asset after), and its objects are read one by one, so a 130 MB artifact
  costs what the preview takes of it. Any model is read this way when its artifact is at least as new as the file and
  its `.meta` (it is exact, and holds a generated second UV set a lightmap needs); an FBX or OBJ whose artifact is older
  or unreadable (one over 600 MB) is read from its file by ufbx as before. A `.blend` (or `.max`, `.ma`, `.mb`, `.c4d`,
  `.lxo`, `.3ds`, `.dae`, `.dxf`, `.skp`) without one cannot be read: the build says "open the project in Unity once".
  Unity may have the project open: everything is only read, and a database that does not add up is no database.
- **Lightmaps** (`scene::world`). Bakery's `ftLightmapsStorage` when the scene has one (it sets the lightmaps when the
  scene loads, so its word counts for the renderers it lists), else each renderer's `m_LightmapIndex` and
  `m_LightmapTilingOffset` with the scene's `LightingData` asset. Each HDR picture (EXR, Radiance HDR, or sRGB) is averaged
  down to 2048 pixels on a side, 1024 above four of them, at most 16, its empty atlas pixels left out of the average and
  filled from their neighbours so that an island's edge does not darken, then stored as a JPEG whose colours read as
  sRGB are the light over a scale (the 99.9th percentile: a hot pixel does not crush the rest). The second UV set is
  a stream of its own, `unorm16` pairs, only on meshes something samples a lightmap with, the first UV set standing in
  when a mesh has none, as in Unity. In the viewer a lit draw's indirect light is its lightmap in place of the ambient
  light, for the scene's light only: the studio's stays the studio's. A baked light (`m_Lightmapping` 2) adds nothing
  more to what samples a lightmap, and still lights what does not. Realtime lights add to it, as Avatorbit's sun does.
- **The sky** (`scene::world::sky`). `RenderSettings.m_SkyboxMaterial`: a latitude-longitude picture (Panoramic, a
  Cubemap imported from one, a shader of its own whose main picture is a whole sphere) at 2048×1024 with its tint,
  exposure and turn; six pictures made into one (6 Sided); Procedural as three colours from its tint and ground colour;
  Unity's default sky as three colours; any other shader the default's colours under its own `_Color`. A sky picture
  also gives the scene's ambient light when that comes from the sky (`m_AmbientMode` 0), which otherwise only Unity
  computes. Drawn behind worlds and scenes in both lights, never behind avatars.
- **Fog.** Unity's three modes, in the scene's light. It starts counting where the framed part of the scene starts: a
  depth cue, never a curtain (Avatorbit's ends at 390 m, and a view of its 120 m town from 400 m away would be all fog).
- **Terrains** (`scene::terrain`). `TerrainData`, binary or text: the heightmap thinned to at most 512 samples on a side
  (half a million triangles), its normals from its slopes; the terrain layers' pictures blended by the splat maps on the
  CPU into one picture of at most 2048 pixels (softer than Unity's up close); trees as their prefab's most detailed LOD
  at each place, at most 4,000 (more are thinned evenly and counted in `stats.skipped.trees`). Grass and other details
  are not drawn and are counted (`stats.skipped.details`). A terrain samples its lightmap by its own 0-to-1 UVs.
- **LOD groups** draw their most detailed level only, in every scene: the others stand in for it from far away, and
  would draw over it.
- **Meshes saved in the scene or a prefab** (ProBuilder and tools' meshes) are read, in text or binary.
- **Pictures.** A world has dozens, most on small props: those that cover the most of it (from their renderers' bounds)
  keep their detail: the eight largest 1024 pixels, the next twenty-four 512, the rest 256. Normal maps are read for
  what is not an avatar, at most 512 pixels. A water shader (by its name; Mochie's in Avatorbit) draws no picture: a
  see-through surface of its emission's tint, its ripples as a normal map, smooth enough for the sun to glint on.
- **Framing.** `focus` is what a preview frames: for a world, all but the outer 3 % on each axis of up to 32 points of
  each drawn object, so a sky dome, a floor reaching for miles or one thing far off do not make the rest tiny (Avatorbit:
  everything 1,000 m across, its town 120 m). A scene that was last open opens from the Scene view's camera in
  `UserSettings/Layouts/*.dwlt` (Unity saves it with the window layout, its pivot, rotation and size), when that lies in
  the scene, drawn back along its line of sight to at least 0.38 of the framed radius when it was left close up on one
  wall. Else over a corner of the framed part.
- **Shadows.** A world's shadow map has 4096² texels (2048² for avatars) and a bias of a tenth of a metre whatever its size;
  no shadow pass is drawn when the key light casts none.

The tail of Ultirex 4.1, in the same work (Enzo: "I haven't touched those files in months"): `orbit-tail-2.fbx` was not
replaced after its import. Its dates are April 2024, and against the artifact Unity made of it in five projects all 52,656
vertices are in the same order at the same place and all 185 bind poses agree to 5e-6. The 7 blend shape weights the
prefab holds are stale values Unity ignores on a mesh without shapes. The fault was the reader's: 1,743 of the mesh's
vertices have no skin weights in the file, Unity binds such a vertex wholly to its first bone (the artifact's weights say so:
`(0, 1.0)`), and the reader left it in the mesh's own space, where a skewed bind pose puts vertices tens of metres out.
The reader now binds them to the bone nearest the root. The workaround (a part posed over 4 m out left out, `skipped.pose`)
is gone: the tail draws, 5 of 25 renderers in Golden ulti against 4, in both reading paths.

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
  again from the new key). A preview without it, from Toolkit 0.4.14 or older, keeps the studio light. There is no
  light switch on the project's card: its cover follows the same choice.
- **A scene whose light would show nothing gets the studio light** (Enzo, 3 November 2026: avatars must never show
  black), whatever the switch says (`preview_gpu::light_mode`, `scene_light_shows`). The rule takes the brightest a
  white matte surface gets from the scene's own light: the brightest channel of the two directional lights drawn,
  summed, plus the brightest of the ambient light's three directions, an unknown ambient light (`null`) counting as
  none. Under `SCENE_LIGHT_MIN` = 0.1 (a tenth of Unity's default sun: a white surface turned to the light stays
  under sRGB 0.35, an avatar's usual colours under about 0.19, hardly above the background's 0.1) the studio's light
  draws it, and the switch's Scene light half is dimmed, not pressable, its tooltip saying why. Measured in Unity
  2022.3.22f1 (linear, never baked): a new scene about 1.2; the default sky without a sun 0.17 (kept, dim and flat as
  in Unity); no skybox and no sun 0.055 (the Lighting window's Ambient Color alone: studio); a scene read by Toolkit
  0.4.15 in the editor update it was opened in, before Unity computed its ambient probe, 0 (`ambient: null`, the
  previews of sunless scenes 0.4.15 wrote: studio). Previews N4N0 builds itself go by the same rule: they record the
  scene's directional lights and a gradient or flat ambient light, never a skybox's (only Unity computes it: `null`),
  and none at all when the scene has neither (the studio light).
- **A baked world's light is its lightmaps**: a preview with `lightmaps` always offers Scene light, whatever its lights and
  ambient light add up to (`preview_gpu::scene_light_shows_header`).
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

Built from the files (3 November 2026; `scene_preview` example, debug build, projects on Enzo's H: hard disk, each
project's last-open scene): 19 of the 19 projects with a scene got a preview (the 20th has none) in 0.1-61 s, 88 MB
of cache for all of them; the same scenes load in the viewer in 34-150 ms. A card's still takes 95-631 ms to draw on
the worker. MCB Test's `Canine.unity` built from the files against Toolkit's exact preview: the same meshes to the
vertex (MCB's binary body 89,166 vertices, 99,642 triangles), the same looks per material and the same bounds to the
millimetre; it is a little brighter, the sky's ambient light being unknown outside Unity (the default sky's stands in).

Worlds, built from the files (4 November 2026; `scene_preview` example, release build, H: hard disk, last-open scenes,
in the project's files only before and with its import's artifacts after):

| Project, scene | | Renderers drawn | Triangles | Materials | Pictures | Build | File |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Avatorbit, `VRCDefaultWorldScene` | before | 113 of 171 | 81,167 | 22 | 7 | 1.2 s | 2.8 MB |
| | after | 420 of 478 | 521,457 | 89 | 39, 7 lightmaps and the sky among them | 3.3-5.9 s | 21.9 MB |
| Avatorbit Quest | after | 486 of 568 | 554,307 | 109 | 43 | 4.7-7.4 s | 24.2 MB |
| Orbital Oween Quest LTCGI | after | 486 of 570 | 554,307 | 110 | 43 | 4.3-7.8 s | 24.2 MB |
| WinMaze | after (2 triangles before) | 6 of 8 | 2,726 | 6 | 2 | 0.5-6.3 s | 0.2 MB |
| Avatorbit, BruteForce's `02_SandTerrain` | after: a terrain | 1 of 1 | 131,072 | 1 | 2 | 0.6 s | 3.5 MB |

Viewer: Avatorbit's preview loads in 350-1,280 ms over eight runs (read 10-16 ms, pictures 320-850 ms on all cores, upload
15-460 ms, the first of a process compiling its pipelines beside it), debug build. Avatars read from their artifacts come out
with the same triangles as from their files (Golden ulti 223,677, MCB 119,708, Ultirex V5 229,203) or within a few of them
(XZavier 180,242 against 180,338, Rexouium 199,678 against 199,684, UltiPaw 252,832 against 252,836); the tail of Ultirex 4.1
adds its part in both. The files in the system's cache, an import
reads faster than its FBX (Kohryu 0.8 s against 2.5, SupaKenji 1.1 against 2.1, XZavier 1.4 against 3.1); the first read of
an avatar's project off the hard disk took 0.4-27 s, within the 120 s a build has.

## Code

| Where | What |
| --- | --- |
| `orbiters_projects::preview` | the format: header types, `read`/`parse` (every blob checked to lie in the file and fit its use), `list` (headers only, newest first), `Writer`, octahedral normals and half floats; tests |
| `pages/projects/preview_gpu.rs` | loading (parallel decode and mipmaps, pipelines on another thread) and drawing: WGSL, shadow pass, mirrored pass, ground, background, MSAA resolved into a texture egui samples (`register_native_texture`), drawn again only when the view changes |
| `pages/projects/preview.rs` | listing (scenes and their previews) and loading jobs, the camera (orbit, goal, inertia, swing-in, reset), the card's cover and its notes, the window and its scene chips, the stand-in scene and the real-project fixtures for screenshots |
| `orbiters_projects::scene` | previews built from a project's files: `scenes`, `list`, `build_scene`, the cache and its freshness (`mod.rs`); `yaml`, `assets` (GUIDs, Unity's LMDB database), `prefab`, `model` (ufbx), `artifact` (Unity's imports in `Library`), `terrain`, `world` (lightmaps, sky, fog, framing, the Scene view's camera), `material`, `texture` (PSD/PSB too, HDR pictures), `mesh_asset`, `primitives`, `build` (posing, budget, writing); `examples/scene_preview.rs` builds real projects' scenes and prints the table below (`--scene <part of its path>`, `--no-artifacts`; `--dump <file> [class]`, `--artifact <project> <guid>`, `--compare <project> <model>` show what a binary file or an import holds, and a model against its import) |
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

`-scene-light` with `ORBITERS_PREVIEW_FILE` naming a preview whose `lighting` is `{ "lights": [], "ambient": null }`
(what Toolkit 0.4.15 wrote for a scene without a sun whose ambient light Unity had not computed) shows it in the studio
light, the switch on Studio light with its Scene light half dimmed.

## Not done or untested

- Toolkit's capture of a real world (only the synthetic world was timed and drawn), and Toolkit writing lightmaps, sky or
  terrains: previews it writes have none of them, so Toolkit's preview of a world is plainer than one built from the
  files. Of a scene's lights only directional ones are used (two of them, the main one's shadow), not point and spot
  lights, light probes (what does not sample a lightmap gets the ambient light), reflection probes, a skybox's picture
  in reflections (its ambient light's three colours stand in), or post-processing; nor occlusion and detail maps, toon
  ramps' pictures, matcaps, rim lights and outlines of toon shaders, specular-setup colours, or metallic and smoothness
  from two different textures (the smoothness one is kept). Terrains' grass and details, holes, and their per-pixel
  blending up close.
- The first capture after a domain reload makes one step of about 27 ms (JIT and first use); later ones stay under
  about 6 ms.
- Non-readable meshes (FBX without Read/Write) were checked readable in Edit Mode without errors (FashionSuit's), and
  a non-readable skinned body bakes; a project in Play Mode or compiling waits.
- The preview needs a Toolkit release with `ProjectPreview` and a desktop release; until then projects show the note.
- Built from the files: `.blend` models of a project never opened in Unity are left out (counted, the reason in the
  cache's record, "open the project in Unity once"): a reader of Blender's own format would still miss its modifiers.
  A compressed mesh in an artifact (`meshCompression` above 0) falls back on the FBX, and is left out for a `.blend`.
  Shaders' looks are Toolkit's rules on the saved properties, not the shaders themselves (a missing shader draws its
  texture rather than Unity's magenta); animators, constraints and what VRCFury or Modular Avatar do at upload are not
  applied (the scene as saved); a skinned mesh's bones are the renderer's; metallic-smoothness and occlusion maps are not
  read, normal maps only for worlds.
