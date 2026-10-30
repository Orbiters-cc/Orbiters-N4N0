# Photo editor

Edit on a picture of Photos & captures opens an editor over the window: the picture is edited as a copy, through a
stack of layers like Lightroom's, and Save writes the copy beside the original. A 3D capture's view can be saved as a
picture from its 3D view (Save this view) and edited the same way. The original is never written.

## Where it opens

- **Picture window** (`pages/sharing.rs`): Edit beside Share publicly / Share to … for a picture on this computer
  (`editor::source_of(entry)`; captures and items only on Orbiters have none). For a VRChat multi-layer picture it opens
  on the picture with its layers (`Entry::layers()`), whichever layer the window shows.
- **3D view** (`pages/splats.rs`): Save this view (camera icon, beside the title) renders the current angle at 2560
  pixels on its long side (at least the view's size) with the poorly seen / far away settings shown, by a renderer of
  its own on the window's device (`still()`: the view on screen carries on; the splats are read again), and writes
  `Orbiters 3D view <date time>.png` into VRChat's pictures folder (its `picture_output_folder`, else `Pictures/VRChat`)
  under the month of the capture, with an XMP packet as VRChat writes one (the capture's world id and name, its
  creation time), so Photos & captures lists it with that world and instance. A shutter flash, then "Saved in Photos &
  captures" with **Edit** (the editor over the 3D view) and Show in its folder. `ORBITERS_EDITOR_PICTURES` sends the
  pictures elsewhere (screenshots).

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
- **Canvas**: the preview as large as fits, over a checkerboard where it is transparent; crop handles; the vignette's,
  the aberration's and a mask's centre (and a circle mask's edge) dragged; texts dragged and chosen by a press on them.
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
| Blur | Gaussian (exact kernel up to σ 3 px, else three box blurs of Kovesi's widths), lens (a disc of running row sums, its edge antialiased, bright points weighted for bokeh; up to 4 % of the height), directional (a line of bilinear taps; up to 10 %) |
| Text | The Toolkit's text layer (`PhotoshootText.cs`): lines centred on a point, a share of the height high, styles Clean, Outline (7.5 % of the size, dark around light letters), Shadow (blurred, offset down-right, black 75 %) and Glow (the colour blurred around brighter letters); outlines from the font file (skrifa) filled by tiny-skia |

Fonts: Inter ExtraBold (default) and the eight OFL fonts My Avatar offers the Toolkit's photoshoot (Bonbon, Creepster,
Diplomata, JetBrains Mono, Matemasie, Monoton, Pirata One, Plaster) are built in (`crates/orbiters-ui/assets/fonts`,
notices in `THIRD_PARTY_NOTICES.md` and `THIRD_PARTY_LICENSES.md`); the computer's installed fonts follow (Windows'
and the user's font folders, read once off the interface thread, those with Latin letters), each name drawn in its
font in a searchable list. Letters a font lacks are left out (no fallback), kerning is not applied.

Crop and rotate: quarter turns and flips, then straighten (±45°) about the middle, then the crop, kept inside the
straightened picture (`geometry::fit`: shrunk about its middle, keeping its shape, so straightening back gives the crop
asked again). Shapes: Free, Original, 1:1, 4:5, 3:2, 16:9, 9:16 (the largest of that shape; the handles keep it). While
Crop and rotate is chosen the canvas shows the straightened picture whole with the crop over it, without the layers.

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

## Files (`editor/files.rs`)

- The copy: `<name> (edited).png` in the original's folder (`(edited 2)`… when taken), RGB when opaque, RGBA otherwise,
  with the original's XMP chunk copied (the world, its name, when it was taken): the gallery lists it with the
  original's world and instance, as a picture of its own (`worlds::shot_name` does not take it for a layer).
- The edit: `<copy's name>.orbiters-edit.json` beside it: `{"format": "orbiters-photo-edit", "version": 1, "source":
  {"picture", "layers": [{"name", "path"}]}, "edit": {...}}`. Every field has a default, so older and newer edits open.
  Opening a copy whose edit and original are there edits it again from the original; Save then replaces that copy.
- `editor::saved()` counts saves; Photos & captures lists its pictures again when it changes.

## Checked

- Unit tests (`cargo test -p orbiters-ui photo`, 31; `cargo test -p orbiters-desktop editor`, 7): sRGB tables and the
  RGBA8 round trip, box downscale; exposure, black point, tone curves monotone at every extreme, brightness/contrast,
  curves through their points, colour; gaussian kernels, box widths, blur light kept and spread by σ, the lens disc and
  bokeh, directional only along its line; vignette around a moved centre, aberration offsets, comic tone steps, dot
  coverage and spacing, ink lines on edges and a layer's outline; quarter turns and flips, crop sampling, straightened
  crops (the analytic largest rectangle), shapes; text placement, every built-in font, styles; the layer order, hidden,
  faded and masked layers, the multi-layer stack order, the kept preview equal to a fresh render; the JSON round trip
  (it found a clash between the effect's tag and the blur's `kind`), multi-layer stacking rules, copy names, XMP carried
  over, reopening; crop handle drags; undo; a whole save (size, edit, original untouched, XMP).
- Screenshots (`orbiters-desktop --screenshot <dir> --only editor --width 1440 --height 900`, with
  `ORBITERS_EDITOR_PICTURE` for the stand-in): `editor-window` (Edit in the picture window), `editor-plain` /
  `editor-open` (before), `editor-light`, `-color`, `-curves`, `-vignette`, `-aberration`, `-comic`, `-blur-gaussian`,
  `-blur-lens` (sharp circle), `-blur-directional` (band), `-text`, `-text-glow`, `-crop`, `-cropped`, `-layers` (the
  add grid), `-multilayer` (world blurred and dimmed, people in the comic look, a title over both).
  `--only editor-3d --sessions <dir>` saves a view of the first reconstructed capture and opens the editor on it
  (`editor-3d-saved`, `editor-3d-edit`).

## Not done or simplified

- Pointer flows (dragging handles, rows and curve points, typing) were checked by staged states, the crop drag math and
  undo by tests, not by hand. No zoom on the canvas.
- Masks are a circle or a band per layer (no brushes). Shadows and highlights are global tone curves, not Lightroom's
  local ones. Text has no kerning, no font fallback for missing letters, no rotation.
- A text's colour is one of twelve swatches (no free picker).
- Save this view reads the capture's splats again for its own renderer (a second copy on the GPU while it renders).
