# 3D preview of a Unity project (29 October 2026)

Enzo asked for a quick 3D preview of a Unity project in Orbiters - N4N0 before opening it, loading in at most about
3 seconds. The Unity projects page now shows the selected project's last saved scene in 3D in its panel's hero card,
in place of the avatar pictures, and grows it into a window over the page on a press. The file format is in contracts'
Docs/project-preview.md.

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
- **Lighting** is the desktop's own, the same for every project: a warm key light from the front left of the avatar
  with a soft shadow (2048² shadow map, 12-tap Poisson PCF), a cool fill, a sky-and-ground ambient, a rim and a small
  highlight, tone-mapped with Khronos PBR Neutral so base colours stay as Unity shows them; 4× multisampling. Avatars
  stand on a floor of our own: a pool of light under them, the shadow, a contact shadow and the scene mirrored faintly
  in it, fading with height and towards the floor's edge into the background. Worlds have their own floors and get
  none.
- **Live.** The project's previews are listed again every 3 s while it is shown, so a save in Unity shows here a few
  seconds later (it rises again where the camera is). Switching back to the project shown just before is immediate.
- **Without a preview** the avatar pictures stay and a quiet chip on the cover says why, with the full explanation on
  hover: "3D after a save" (Orbiters Toolkit is in the project: save the scene once), "No 3D preview" (Toolkit is not
  in it: add it below, then save), or "3D preview unreadable" with the reason. A spinner shows while it loads.

## Why a snapshot written by Toolkit

Nothing outside Unity can draw a project's scene cheaply: scenes reference prefabs, FBX models, materials of custom
shaders (Poiyomi, lilToon, locked variants) and textures whose usable forms live in `Library`; posing an avatar needs
its skinning, blendshape weights and constraints. Reproducing that in the desktop would be a second Unity importer,
slow on first open and never quite right. Inside Unity all of it is a few calls away, so Toolkit keeps a small
ready-to-draw copy (`UserSettings/Orbiters/Previews/<scene GUID>.preview`): posed meshes, base-colour pictures (JPEG,
PNG where alpha matters) and where everything stands. The desktop reads the file in one go and uploads its vertex and
index blobs as they are; it converts nothing. A project never opened with such a Toolkit has no preview and says so.

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
| `pages/projects/preview.rs` | listing and loading jobs, the camera (orbit, goal, inertia, swing-in, reset), the card's cover, the window, the stand-in scene for screenshots |
| Toolkit `Editor/Preview/ProjectPreview.cs`, `PreviewFile.cs` | the capture and the writer; `ProjectPreview.CaptureNow`, `LastResult` for diagnostics |

The texture egui paints is the resolved picture, so the card's squircle corners, the crossfade over the avatar
pictures and the window's morph come from the usual painters; its size grows with the window and shrinks once the
window is back in the card.

## Screenshots

```powershell
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-preview
$env:ORBITERS_PREVIEW_FILE = "<a .preview Toolkit wrote>"   # instead of the stand-in scene
```

Scenarios: `projects-preview` (settled), `-reveal` (rising, 0.5 s in), `-swing` (1 s in), `-turned` (dragged),
`-open-mid` (the window six frames into its morph), `-open`, `-scenes` (two scenes to switch between), `-sheet`
(with `--width 1040`), `-none` and `-no-toolkit` (the notes). The stand-in is N4N0's black orb with white strokes over
a plinth, a green ring and two cubes, as an avatar scene.

## Not done or untested

- A real world project's capture (only the synthetic world was timed and drawn); terrains, lightmaps, emission and
  normal maps are not in the format.
- The first capture after a domain reload makes one step of about 27 ms (JIT and first use); later ones stay under
  about 6 ms.
- Non-readable meshes (FBX without Read/Write) were checked readable in Edit Mode without errors (FashionSuit's), and
  a non-readable skinned body bakes; a project in Play Mode or compiling waits.
- The preview needs a Toolkit release with `ProjectPreview` and a desktop release; until then projects show the note.
