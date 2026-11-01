# Photo editor

Edit on a picture of Photos & captures opens an editor over the window: the picture is edited as a copy, through a
stack of layers like Lightroom's, and Save writes the copy beside the original. A 3D capture's view can be saved as a
picture from its 3D view (Save this view) and edited the same way. The original is never written.

## Where it opens

- **Picture window** (`pages/sharing.rs`): Edit beside Share publicly / Share to … for a picture on this computer
  (`editor::source_of(entry)`; captures have none). For a VRChat multi-layer picture it opens on the picture with its
  layers (`Entry::layers()`), whichever layer the window shows. A picture only on Orbiters (shared with the member, or
  theirs from another computer) has Edit too, beside Download: see [Pictures of Orbiters](#pictures-of-orbiters).
- **Gallery picture window** (`pages/community.rs`, a community's or an event's gallery in Photos & captures,
  Communities and an event's window, and an asset's pictures of My assets): Edit over the picture's bottom right, the
  editor over the whole window (`editor::gallery_source`; an asset's picture by its link).
- **A card's menu** in Photos & captures (a right press): Edit for a picture on this computer (its window, then the
  editor) and for every picture from Orbiters (shared, a gallery's, an asset's: the editor at once, over the page; a
  picture already fetched for a drag or a copy, `Selection::fetched`, is put in place instead of downloaded again,
  `files::seed`). Not for a 3D capture's picture.
- **3D view** (`pages/splats.rs`): Save this view (camera icon, beside the title) renders the current angle at 2560
  pixels on its long side (at least the view's size) with the poorly seen / far away settings shown, by a renderer of
  its own on the window's device (`still()`: the view on screen carries on; the splats are read again), and writes
  `Orbiters 3D view <date time>.png` into VRChat's pictures folder (its `picture_output_folder`, else `Pictures/VRChat`)
  under the month of the capture, with an XMP packet as VRChat writes one (the capture's world id and name, its
  creation time), so Photos & captures lists it with that world and instance. A shutter flash, then "Saved in Photos &
  captures" with **Edit** (the editor over the 3D view) and Show in its folder. `ORBITERS_EDITOR_PICTURES` sends the
  pictures elsewhere (screenshots). **Edit in 3D** beside it edits the view live while moving around ([Editing in
  3D](#editing-in-3d)).

## The editor (`pages/editor/`)

- **Top bar**: back (asks once when changes are not saved), the picture's world and file, Undo / hold-to-compare /
  Redo, Save a copy (Save for a copy opened again) with its progress.
- **Layers** (left): Crop and rotate pinned on top, then the layers top first (the last applied on top), each with its
  eye, kind, and what its settings come to ("+0.45 EV, shadows +55", "Radius 1.8 %, bokeh 80 %"), its opacity
  or mask; the picture locked at the bottom. A multi-layer picture shows Whole picture, then each of VRChat's layers
  (Interface, People, World) with its own eye and its effects under it. Rows are chosen by a press, dragged up and down
  their stack, removed by their bin (or Delete). Add opens the eight kinds; a new layer goes over the chosen one, in its
  stack (a new text says the world's name).
- **Controls** (right): opacity, the kind's own controls, then Where (Everywhere, Circle, Band, inverted or not, with
  radius / width, softness and angle). Move up / down, Duplicate and Remove in its header.
- **Canvas**: the preview as large as fits, over a checkerboard where it is transparent; the vignette's, the
  aberration's and a mask's centre (and a circle mask's edge) dragged; texts dragged and chosen by a press on them.
  Crop and rotate turns the canvas into the iPhone's crop ([below](#crop-and-rotate-editorcroprs)).
- **Colour and tone sliders** (`editor/widgets.rs`, `Look`): each track is drawn as what it does, left to right, with
  a small icon in its colour beside its name and a knob showing the colour it is on: exposure and brightness dark to
  light (±/sun), contrast a grey parting into black above and white below (half circle), highlights the light tones
  (dim sun), shadows the dark tones (moon), black point a faded black to a deep one (a black dot), temperature blue to
  amber (thermometer, its colour following the value), tint green to magenta (drop), saturation and vibrance grey to two
  vivid hues (half drop, sparkle), hue the hues red turns to (rainbow, in the hue chosen), the vignette's amount
  lighter edges to dark (aperture), aberration grey to red above and blue below, the comic's ink lines light to black.
  The curves draw the tones in and out along their sides, black to the channel's colour. Opacity, sizes, softness and
  angles keep the accent fill.
- Keys: Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, Delete, Escape (closes the add list, then the selection, then the editor).
  Undo keeps one step per drag or per pause in typing (200 steps).

### Effects (`orbiters_ui::photo`)

Pixels are linear light, premultiplied alpha, f32 (`Image`); what reads like a print works on encoded values. Sizes are
shares of the picture (of its height for lengths), so the preview and the saved picture look alike.

| Layer | What it does |
|---|---|
| Light | Exposure in stops (linear), then a tone curve on encoded values: black point (below zero lifts black to a faded grey), brightness (a gamma), contrast (towards a smoothstep), shadows and highlights (shaped lifts that never fold the tones over) |
| Color | Temperature and tint as white balance gains, hue (the CSS hue-rotate matrix), saturation and vibrance around Rec. 601 luma |
| Curves | A curve for RGB and one per channel, monotone cubic through the points (Fritsch–Carlson, no overshoot), channel then RGB; the editor draws it over the preview's luma histogram |
| Vignette | The Toolkit's `pow(saturate(1 - d·d·1.8), 2.2)` around a centre that moves, size and softness added; below zero it lightens the edges |
| Chromatic aberration | The Toolkit's: red sampled `amount × 0.012` of the way towards the centre, blue as far away (amount 0 to 3) |
| Comic | The Toolkit photoshoot's halftone pass (`PhotoshootEffects.shader`, `Comic`), on encoded values: the luma stepped to 2–12 tones with the hue kept and a 1.2 saturation boost, print dots on a 45° grid whose area is the darkness (cell from a 260th to a 36th of the height), ink lines (0.06, 0.05, 0.08). The Toolkit finds lines in the scene's depth and normals; a picture has none, so lines come from the jumps of a softened luma across a line width (a 520th of the height) and, stronger, of alpha: on VRChat's People layer the silhouettes are inked |
| Blur | Gaussian (exact kernel up to σ 3 px, else three box blurs of Kovesi's widths), lens (a disc of running row sums, its edge antialiased, bright points weighted for bokeh; up to 4 % of the height), directional (a line of bilinear taps; up to 10 %); depth of field on a 3D view's splats ([below](#depth-of-field)), nothing on a picture's pixels |
| Text | The Toolkit's text layer (`PhotoshootText.cs`): lines centred on a point, a share of the height high, styles Clean, Outline (7.5 % of the size, dark around light letters), Shadow (blurred, offset down-right, black 75 %) and Glow (the colour blurred around brighter letters); outlines from the font file (skrifa) filled by tiny-skia |

Fonts: Inter ExtraBold (default) and the eight OFL fonts My Avatar offers the Toolkit's photoshoot (Bonbon, Creepster,
Diplomata, JetBrains Mono, Matemasie, Monoton, Pirata One, Plaster) are built in (`crates/orbiters-ui/assets/fonts`,
notices in `THIRD_PARTY_NOTICES.md` and `THIRD_PARTY_LICENSES.md`); the computer's installed fonts follow (Windows'
and the user's font folders, read once off the interface thread, those with Latin letters), each name drawn in its
font in a searchable list. Letters a font lacks are left out (no fallback), kerning is not applied.

Crop and rotate (`photo::geometry`): quarter turns and flips, then the perspective tilts, then straighten (±45°) about
the middle, then the crop, kept inside the tilted and straightened picture (`geometry::fit_warp`: shrunk about its
middle, keeping its shape, so undoing the tilt and the straightening gives the crop asked again). The tilts (Vertical
and Horizontal, ±45°, `Geometry::vertical` / `horizontal`) are a homography (`geometry::Warp`): the picture's plane
turned in depth about its middle line in front of a camera 0.75 of its long side away (`FOCAL`, about a 67° view) and
seen again from that camera, `[[f·cosφ, f·sinφ·sinθ, 0], [0, f·cosθ, 0], [−sinφ, cosφ·sinθ, f]]` about the middle
(θ vertical: positive brings the top nearer and wider; φ horizontal: positive brings the right side nearer and taller).
The middle stays put; walls leaning together towards a point d above the middle stand upright at sin θ = f / d (for a
camera turned up by α, sin θ = tan α). Its inverse (the adjugate) samples the source; a frame point beyond the tilted
picture's horizon shows nothing. The picture's outline in the frame is convex, so its four corners say whether a crop
fits. Shapes (`Aspect`): Freeform, Original, Square and the iPhone's 9:16, 4:5, 5:7, 3:4, 3:5, 2:3 standing, or lying
(16:9, 5:4, 7:5, 4:3, 5:3, 3:2, the original turned); the largest of that shape, the handles keep it.

AUTO (`photo::level`): the picture is looked at 480 pixels on its long side; Sobel gradients (those leaning at most 25° from
level or upright, stronger than a tenth of the 95th percentile's: a picture's lit windows must not hide its walls) vote in a Hough accumulator (quarter degrees by pixels from the middle)
for the lines within 3° of their gradient; its peaks (a sixth of the short side long, the highest within 2° and 3
pixels, refined between angle steps by a parabola) are the lines, upright ones weighing 0.75 of level ones (a building
from below leans its walls together); a histogram of their leans spread over a degree finds the one most of their
length agrees on, and the mean of the lines within a degree of it is the tilt (at least a fifth of the length must
agree, else nothing is found). Straightening by its opposite levels the picture (the other way when flipped once).

### Crop and rotate (`editor/crop.rs`)

As the iPhone's Photos crops, over the canvas:

- **Top**: Cancel (the geometry as it was when the crop opened) and Reset (shown while the crop is not the picture as
  taken), Done on the right (lit only once something changed; Escape is Done too). Between them: flip across, rotate
  a quarter to the left, **AUTO** (a pill, filled with the accent while the straightening is the one found; pressed
  again it goes back to level; dim when no lines were found, its tooltip says by how much), the shapes, and **…**
  (rotate right, flip upside down, copy this crop, paste the crop copied, undo every change of the crop). Turns and
  flips keep what shows: the crop, its shape, the flips and the tilts turn with the picture; a flip mirrors the crop,
  the straightening and the tilt across it.
- **The picture** under a white frame with thick corners just outside it and marks at the middle of its sides, the rest
  dimmed (darker at rest, lighter while it moves); thirds while it is dragged, a ninth grid while it is straightened.
  The frame's corners and edges resize it as far as the picture goes (across first, then down): the view holds still
  under the pointer, then springs to the crop centred and as large as fits. A drag inside moves the picture under the
  frame (the crop stays where it is on screen); the wheel or a pinch zooms the picture about the pointer, the frame
  holding still. The view is a spring of its own (`View`: held, snapped or animated) towards `target(area, crop)`; the
  preview is asked at the zoomed frame's size in half steps of two.
- **The dials**: Straighten, Vertical, Horizontal, round: the chosen one shows its value (rounded) in a ring that fills
  with it (clockwise for positive, a full turn at 45), the others their icon (a level line, a frame leaning together
  upwards, to the left), accent while not zero. A press on the chosen one sets it to zero, the next puts it back. Under
  them its name and a **ruler** of degrees (8 points each, longer every 5, a dot over 0) scrubbed left and right: it
  coasts when let go at the speed it had (slowing by e^−4.5t), holds at zero for 1.2° of travel as it passes (the
  middle mark ticks), stops there when coasting slowly, double press for zero. Changes while it coasts are one step of
  undo.
- **The shapes** (the shapes button): portrait or landscape, then Original, Freeform, Square, 9:16, 4:5, 5:7, 3:4, 3:5,
  2:3 (or their landscape forms) in place of the dials.
- The right panel has the same: orientation and shapes, Straighten / Vertical / Horizontal sliders, the quarter turns
  and flips, Reset and the copy's size.

The parts are functions of the frame's place and the geometry (`target`, `View::follow`, `overlay_input`, then the
picture, then `draw_overlay`): Edit in 3D crops its live view with them (its view drawn in the slot left for it).

VRChat multi-layer pictures (`Entry::layers()`: `…_Environment.png`, `…_Player.png`, `…_UI.png` beside the picture):
each layer is a stack of its own, stacked back to front before the whole picture's layers. With the world among them,
the layers alone make the picture; without it, VRChat's composite is the bottom ("Picture") and the other layers go
over it (their effects then cover the composite's own people).

### Rendering

`photo::render::Renderer` crops and turns the sources at the size asked (bilinear in linear light), runs each stack,
and keeps between previews the turned sources and the picture just before the layer being changed: a slider drag redoes
that layer and those over it. The preview runs on a thread of its own (`editor/preview.rs`) from the pictures scaled to
2048 pixels on their long side, at about the canvas's pixels; a newer request stops the one being made at its next
layer. Save reads the files again at full size and renders on a job with progress (`editor::save`).

Measured on a 3840×2160 picture in a debug build (`orbiters-ui` is optimised in debug builds), each layer with the
crop: light 85 ms, colour 77, curves 115, vignette 63, aberration 107, comic 220, gaussian 173, text 65, lens 823,
directional 1461; all eleven at once 2.9 s; their 1400×788 preview 196 ms
(`cargo test -p orbiters-ui photo::render::tests::a_4k_picture -- --ignored --nocapture`).

## Editing in 3D

**Edit in 3D** (beside Save this view in a capture's 3D view, `pages/splats/edit3d.rs`) opens the editor over the
live 3D view: the layers and controls as for a picture, and on the canvas the view still drawn and steered as in the
3D view (drag to orbit, right-drag to pan, scroll to zoom, WASD and QE to fly, double-click for the recorded view). A
press on a handle (a vignette's or aberration's centre, a mask, a text) moves the handle; anywhere else it moves the
camera. Every layer of the stack applies to each frame: light, colour, curves, vignette, chromatic aberration, comic,
the three blurs, text, masks and opacity, and crop and rotate (straighten and the perspective dials included; Crop and
rotate shows the whole frame with the crop editor over the live view). Sizes and places are shares of the picture, so
the layers stay where they are on it while the camera moves. Leaving (back, Escape) keeps the layers for the next
time; the eye at the top shows the view without them while held.

- **The picture**: the view's shape when the mode opened, 2560 pixels on its long side when saved (`STILL_LONG`, at
  least the view's size). On the canvas the splats are drawn at the canvas's pixels of the picture shown (more of the
  view when cropped, up to the full size); on the integrated GPU a moving camera draws fewer pixels as the 3D view does.
- **On the GPU** (`orbiters_ui::photo::gpu`, `gpu.wgsl`): each layer is one compute pass or a few over half-float
  textures in linear light with premultiplied alpha, ported from the CPU passes with the same maths: the light and curve
  tables (1025 entries), the white balance, hue matrix and vibrance, the vignette's and aberration's formulas, the
  comic's tones, dots and ink lines (its softened luma blurred like the CPU's), the gaussian's exact kernel up to σ 3
  and Kovesi's three box blurs beyond, the lens's disc from running sums along the rows (one thread a row) with its
  antialiased ends and bokeh weights, the directional blur's taps, the mask and opacity mix; texts are drawn on the CPU
  as for a picture (kept until the text or the picture's height changes) and laid over. Crop, turns, flips,
  straightening and perspective are one homography through the frame's corners (`Geometry::source_point`), sampled
  bilinearly in linear light as `geometry::transform` does. A frame of the view is drawn through the whole stack; with
  the camera still, a slider redoes the layer being changed and those over it from the picture kept before it, and
  nothing is done while nothing changes. The paint callback draws the splats as the 3D view does (or takes the
  integrated GPU's frame), runs the stack on the window's device and blends its picture over the canvas.
- **Save picture** renders the current angle at full size (as Save this view does, the poorly seen and far away
  settings shown), saves it as `Orbiters 3D view <date time>.png` with the capture's world in its XMP, puts the layers
  on it on the GPU and writes the edited copy beside it, `… (edited).png`, with the edit in its
  `.orbiters-edit.json` and the camera under `view` (the capture, the orbit's pivot, turn, tilt, distance and field of
  view, the eye and where it looks, the picture's size, how poorly seen and far splats showed). Each save is a picture
  of its own. **Save this view** (in the top bar) still writes the view without the layers.
- **Back to 3D**: the editor opened on such a copy (from Photos & captures, or Edit after saving a view) has **Edit in
  3D** in its top bar while the capture is there: it opens the capture's 3D view where the camera was, with the
  display settings it had, in Edit in 3D with the copy's layers and crop.
- **Depth of field** (a Blur layer's "Depth of field" switch, offered in Edit in 3D): see [below](#depth-of-field).
- **Looks** (the sparkle button left of Save, in both editors, `editor/looks.rs`): the layers kept under a name ("Look
  1"…, in `looks.json` in the settings folder; the crop is the picture's own and is not kept), applied over the layers
  of another picture or of the 3D view, removed from the same list.

Matching the CPU (`cargo test -p orbiters-ui photo::gpu`): each kind of layer and its variants (every light slider, a
faded black, a hue turn, grey, curves per channel, a moved lighter vignette, strong aberration, comic with dots and
without ink, the box path of the gaussian, the lens with bokeh and small, directional, glow and shadow texts), alone,
whole and faded through a circle or an inverted band, on an opaque picture and one with a transparent corner, drawn on
the GPU and by `render::Renderer`: at most 1 of 255 apart everywhere but the comic, whose tone steps and ink threshold
fall the other way on 0.01 % of the pixels (0.3 % with its dots). The geometry (quarter turns, flips, straightening,
two perspective tilts, crops) and the stages without layers are at most 8 apart on a few edge pixels (mean under 0.3).
A live stack redoes only the layer changed and draws what a fresh run draws, byte for byte. Timed with
`cargo test -p orbiters-ui photo::gpu::tests::a_view_sized -- --ignored --nocapture` on a 1600×900 picture, each layer
including its submission and read back: 0.5 to 1.7 ms; the twelve at once 4.6 ms.

## Files (`editor/files.rs`)

- The copy: `<name> (edited).png` in the original's folder (`(edited 2)`… when taken), RGB when opaque, RGBA otherwise,
  with the original's XMP chunk copied (the world, its name, when it was taken): the gallery lists it with the
  original's world and instance, as a picture of its own (`worlds::shot_name` does not take it for a layer).
- The edit: `<copy's name>.orbiters-edit.json` beside it: `{"format": "orbiters-photo-edit", "version": 1, "source":
  {"picture", "layers": [{"name", "path"}]}, "edit": {...}}`. Every field has a default, so older and newer edits open.
  Opening a copy whose edit and original are there edits it again from the original; Save then replaces that copy.
  A copy saved in Edit in 3D has `"view"` too (the capture and its camera): Edit in 3D goes back there.
- `editor::saved()` counts saves; Photos & captures lists its pictures again when it changes.
- Geometry fields added since the first version (`vertical`, `horizontal`, the new shapes' names, `"source":
  {"remote"}`) have defaults; the first version's shapes keep their names (`"portrait"` is 4:5, `"classic"` 3:2,
  `"wide"` 16:9, `"story"` 9:16). An edit with a new shape does not open in the first version (its copy opens as a
  picture there).

### Pictures of Orbiters

A picture shared with the member (Photos & captures' Shared with me, or theirs only on Orbiters), a picture of a
community's or an event's gallery and an asset's picture are edited as a copy of their full picture (`files::Remote`, `Source::remote`):

- **Download** (`files::fetch`, on the editor's loading job, "Downloading the picture at its full size…"): a shared
  picture's file (`/captures/picture/:id/file`, the member's credential), a gallery picture's full picture by the link
  `/galleries/images/:id/source` renews (asked again with `failed` when the first does not load; Discord's links
  expire; the link listed when Orbiters renews none), an asset's picture by its public link (`RemoteKind::Link`, no
  account needed), up to 96 MB, into `<cache>/editor-pictures/<shared|gallery>-<dev|prod>-<id>.img` or
  `link-<hash of the link>.img` (once: opened again, it is read from there). Where a card of Photos & captures is
  fetched from is the gallery's own rule (`gallery_select::remote`), so the editor fetches what a drag would. The
  server is the one the app is on. The canvas keeps a middle press (`autoscroll::keep_middle`).
- **Save a copy** writes into VRChat's pictures folder (as Save this view does), in the month it was taken, named
  after it (`<event or world> by <who> (edited).png`), with an XMP packet as VRChat writes one: the world (the shared
  item's, the gallery event's world) and when it was taken (the item's time, the gallery picture's posting time, else
  the event's start). Photos & captures lists it at once (`saved()`) with that world, in the instance the member was in
  then when VRChat's logs say so, else by world and day. Its edit beside it points at the download: opened again, it
  edits again from it.

## Checked

- Unit tests (`cargo test -p orbiters-ui photo`, 43 with the GPU's; `cargo test -p orbiters-desktop editor`, 16): sRGB tables and the
  RGBA8 round trip, box downscale; exposure, black point, tone curves monotone at every extreme, brightness/contrast,
  curves through their points, colour; gaussian kernels, box widths, blur light kept and spread by σ, the lens disc and
  bokeh, directional only along its line; vignette around a moved centre, aberration offsets, comic tone steps, dot
  coverage and spacing, ink lines on edges and a layer's outline; quarter turns and flips, crop sampling, straightened
  crops (the analytic largest rectangle), shapes; text placement, every built-in font, styles; the layer order, hidden,
  faded and masked layers, the multi-layer stack order, the kept preview equal to a fresh render; the JSON round trip
  (it found a clash between the effect's tag and the blur's `kind`), multi-layer stacking rules, copy names, XMP carried
  over, reopening; crop handle drags; undo; a whole save (size, edit, original untouched, XMP).
- The crop (`photo::geometry`, `photo::level`, `editor::crop`, `editor::files`): the tilts are homographies about the
  middle (identity untilted, the middle kept, there and back, lines kept straight), a vertical tilt's verticals meet f /
  tan θ below the middle and a horizontal one's horizontals f / tan φ to the left, walls leaning towards a point d
  above stand upright at sin θ = f / d; a tilted crop is the largest inside (1 % more is not), its pixels the source's
  under the warp, the corners beyond a tilt empty; the presets standing and lying in the iPhone's order, each the
  largest of its shape; AUTO finds a tilted horizon (5°, −3.5°, 12°, 0°, −20°, within 0.3°), walls leaning 4°, a −2°
  horizon among walls leaning together, nothing in a soft blob nor a 40° diagonal, and its straightening levels the
  line; handles and the picture moved under the crop stop at the picture's edge, zoom keeps the point under the
  pointer, the view centres the crop as large as fits, four turns or two flips give the geometry back and a turned
  flipped picture shows the same corner, AUTO the other way when flipped; an edit as the first version saved it opens
  with its shape and no tilt and writes back the same, new shapes and tilts round trip; a picture of Orbiters' cache
  file, copy name and month folder, XMP and JSON, and its save (world, name and time read back by the gallery's
  reader, reopened from the download).
- Screenshots (`orbiters-desktop --screenshot <dir> --only editor --width 1440 --height 900`, with
  `ORBITERS_EDITOR_PICTURE` for the stand-in): `editor-window` (Edit in the picture window), `editor-plain` /
  `editor-open` (before), `editor-light`, `-color`, `-curves`, `-vignette`, `-aberration`, `-comic`, `-blur-gaussian`,
  `-blur-lens` (sharp circle), `-blur-directional` (band), `-text`, `-text-glow`, `-crop`, `-cropped`, `-layers` (the
  add grid), `-multilayer` (world blurred and dimmed, people in the comic look, a title over both); the crop:
  `editor-crop` (4:5, straightened 6°), `-crop-auto` (the skyline drawn here turned 4°, AUTO lit, levelled at −3.8°),
  `-crop-vertical` (+14°), `-crop-horizontal` (−12°, straightened 2.5°), `-crop-shapes` (landscape, 4:3),
  `-crop-zoomed` (a square zoomed off the middle); `editor-remote` (a gallery picture, its download standing in);
  `gallery-community-open` has Edit over the gallery picture.
  `--only editor-3d --sessions <dir>` saves a view of the first reconstructed capture and opens the editor on it
  (`editor-3d-saved`, `editor-3d-edit`).
- Edit in 3D: `cargo test -p orbiters-ui photo::gpu` (the GPU against the CPU, above), `cargo test -p orbiters-desktop
  editor` (a copy saved in 3D asks for its view again, a live editor asks its host to save, looks kept by name and put
  over another edit) and `splats` (the view in the edit's JSON). `--only editor-3d-live --sessions <dir>`: the 3D view
  with its Edit in 3D button (`edit3d-view`), the mode opened (`edit3d-open`), light, colour, aberration, vignette and a
  glowing title over the live view at one angle and the camera turned (`edit3d-angle-1`, `edit3d-angle-2`), crop and
  rotate over it (`edit3d-crop`), the comic look with a lens blur outside a circle (`edit3d-comic`), the Looks menu
  (`edit3d-looks`) and Save picture done (`edit3d-saved`; the view, its edited copy and the edit with the camera are
  written into `<out>/editor-pictures`).

### Depth of field

A Blur layer with **Depth of field** on (`BlurKind::Depth`, offered in Edit in 3D) blurs the splats themselves, each by
its own distance from the focus, on every frame as the camera moves (`orbiters_reconstruct::dof`,
`shaders/project.wgsl`). A thin lens: a point `z` metres along the view, through an aperture `A` metres across focused
at `s` metres, spreads over a disc `fy · A · |1/s − 1/z|` pixels across (`fy` the camera's focal length in pixels, so
the disc is the same share of the picture at any size; at most 12 % of the height). In the projection pass each splat's
2D covariance grows by that disc's variance (`r²/4` along each axis for a disc of radius `r`) and its opacity falls by
`√(det before / det after)`, so its light is kept (as the confidence control's blur and the 2D filter keep it). No pass
over the splats on the CPU, no extra GPU pass.

- **Controls**: Focus **Auto** or **Manual**; Aperture (0 to 100 %, 100 % = 0.2 m across: a subject 2 m away in a 60°
  view puts the far background in discs about 8 % of the height), Foreground and Background (how much of the disc shows
  in front of the focus and behind it). The layer's opacity scales the aperture; it has no Where. The topmost shown depth
  blur counts (`Edit::depth_blur`). Its row says "Auto focus, aperture 70 %" or "Focus 6.0 m, aperture 70 %".
- **Auto**: a focus dot on the canvas (a ring with four ticks, the distance beside it), dragged like a vignette's centre
  (`Blur::point`, a share of the picture: taken back through the crop and tilts to the view's pixel). The same projection
  pass adds each splat's optical depth at that pixel, `−ln(1 − α)` with α as the raster blends it before any blur, into
  256 bins of depth (log-spaced, 5 cm to 2 km, fixed point in a 1 KB buffer). Light through splats keeps `Π(1 − α)` in
  any order, so the bins say without sorting where what is under the dot has hidden half of what is behind it (`dof::
  focus_of`): the nearest surface that dominates the dot, a faint veil in front passed through; the sky or nothing:
  far away (1 km). The histogram comes back through a mapped buffer a frame or two later, never waited for
  (`Gpu::probed`); the focus eases towards each answer in the log of the distance (about 0.12 s). Since the probe reads
  unblurred alphas, the blur never moves its own focus. **Keep this distance** turns the found focus into a manual one.
- **Manual**: Distance (10 cm to 500 m along a log slider). While it is dragged, with **Show the focus while setting it**
  on, the splats within about 4 % of the distance are tinted green.
- **Saved**: with the edit as other layers (`"kind": "depth"`, `"focus": "auto" | "manual"`, `"point"`, `"distance"`,
  `"aperture"`, `"near"`, `"far"`; an edit without them has no depth of field). Save picture renders the view twice at
  full size: the original without it (as Save this view), and the one its layers go over with it (the auto focus where it
  is when saved). Compared (the eye held) the view shows without it.
- **Cost** (`ORBITERS_DOF_PLY=<splats.ply> cargo test -p orbiters-reconstruct --lib gpu::tests::depth_of_field_frame_time
  -- --ignored --nocapture`, 1600×900, RTX 4080, each frame waited for, four directions from the splats' median): Palm
  Point (745 k splats) 1.8–2.0 ms off, 2.0 ms at 40 %, 2.1 ms at 100 %, 2.25 ms at 100 % focused at 0.8 m, 2.2 ms with
  the probe; New Generation (970 k) 2.0 ms off, 2.1, 2.4, 2.7, 2.2. The wider splats touch more tiles (up to twice the
  entries focused near at full aperture).
- **Checked**: `cargo test -p orbiters-reconstruct dof` (the disc is zero at the focus and grows on either side, the thin
  lens's size, the cap, no aperture changes nothing, a blurred splat keeps its weight, the focus at half the light past a
  veil, faint splats adding up, nothing: far away, easing), `gpu::tests::a_viewer_blurs_splats_by_their_depth` (on the
  GPU: the splat at the focus unchanged, the other wider, fainter and its light within 20 %, foreground or background
  alone, the focus tinted) and `the_focus_probe_finds_the_dominant_surface_under_a_pixel` (a wall behind a faint veil, a
  far splat, empty sky, unchanged by a strong blur); `cargo test -p orbiters-desktop editor splats` (its JSON round trip,
  old edits with none, a picture left as it is, the topmost shown one with its opacity, one undo step per drag, the lens
  and the dot's pixel through a crop). Screenshots: `--only edit3d-depth --sessions <dir>`: `edit3d-depth-off`,
  `-auto` (the dot in the middle, 14 m on Palm Point: the posts in front blurred), `-auto-moved` (the dot on a near
  post, 4.6 m: the pergolas blurred), `-manual-near` (1.5 m), `-manual-far` (25 m), `-plane` (6 m tinted), `-saved`.

## Not done or simplified

- Edit in 3D: one source (VRChat's multi-layer stacks are for pictures); the curves draw no histogram over the live view;
  the canvas has no zoom; the view's shape is the one it had when the mode opened (a window resized later fits it
  letterboxed). The comic's tones and the GPU's half floats differ from the CPU's on a few pixels (see above), so a
  copy saved in 3D and opened again in the picture editor (which renders from the view on the CPU) can differ there.

- Pointer flows (dragging handles, rows and curve points, typing) were checked by staged states, the crop drag math and
  undo by tests, not by hand. No zoom on the canvas outside the crop. The crop's handle drags, pan, wheel zoom, the
  ruler's scrub, coasting and notch, the "…" menu and a real download of a shared or gallery picture were not tried by
  hand (staged states and tests only).
- The crop does not wait a moment before re-centring after a handle is let go, as the iPhone does; there is no haptic
  tick (the ruler's middle mark flashes). AUTO straightens only (no automatic perspective), and on a picture without
  long straight lines it finds nothing. The tilts are about the picture's middle lines (a virtual camera 0.75 of the
  long side away), not the camera's own lens.
- A copy of a picture of Orbiters carries its world and time, not its instance: Photos & captures puts it in the
  instance VRChat's logs had the member in then, else by world and day.
- Masks are a circle or a band per layer (no brushes). Shadows and highlights are global tone curves, not Lightroom's
  local ones. Text has no kerning, no font fallback for missing letters, no rotation.
- A text's colour is one of twelve swatches (no free picker).
- Save this view reads the capture's splats again for its own renderer (a second copy on the GPU while it renders).
- Depth of field: a picture has no depth, so a depth blur draws only in Edit in 3D; a copy saved there and opened in the
  picture editor (which renders from the view without it) loses it when saved from there (the layer says so). The blur
  is a Gaussian as wide as the lens's disc (no bokeh shapes); splats still blend in their order from the eye, so a
  blurred foreground does not spread over what is behind it as a lens would show it, and a dense surface blurred a lot
  lets a little of what is behind it through. The auto focus follows one pixel (no area), and the plain 3D view (outside
  Edit in 3D) has no depth of field.
