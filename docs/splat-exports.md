# Export a reconstruction

## From the app

**Export…** turns a capture into a standard splat file for other tools. It is where captures are already acted on: the 3D
view's top bar (beside Select splats), the capture's window in Photos & captures (beside Share to … and Copy public
link), its card's menu there, and its detail in Capture files (beside Send to Mixer). A dialog grows out of the button:

| Format | Says | For | Size (Home, 731,087 splats) |
|---|---|---|---:|
| SPZ (version 3) | Small | VRChat splat worlds (VRChat Gaussian Splatting), Unity splat importers, SuperSplat, Spark, web viewers | 12.9 MB |
| GLB (`KHR_gaussian_splatting`) | glTF standard | Cesium, three.js and other glTF viewers that draw splats; Unity's and Blender's own importers cannot open it yet | 40.9 MB |
| PLY (reference 3DGS layout) | Full precision | every splat tool, Blender's splat add-ons | 49.7 MB |

Each row gives the file's size before it is made. Windows' save dialog offers the capture's folder name (its world and when
it was recorded) and opens where the last export went; the export runs off the interface thread and says how far it got
(reading, making the file, writing; closing the dialog stops it). It ends with where the file was saved and Show in
folder. SPZ adds a note when more than 0.1% of its colour values were clamped because they were brighter or darker than
its 8 bits hold (Home: 6.1%). GLB and PLY keep every value.

**Every format holds what the 3D view shows.** Splats hidden or deleted with Select splats are left out, as from shares
and from Mixer's copy. The PLY leaves them out too: the person exports what they see, and a PLY with a removed avatar in
it would bring it back in every tool it goes through. None holds the sky: no other tool draws N4N0's sky map, and the sky
shell's discs, 400 m away, would wall a world in.

Failures say why and give a way on:
- Splat edits that cannot be applied (damaged, or made on an earlier reconstruction) stop the export, as they stop
  shares; Export waits and the dialog offers **Discard them** (the 3D view's own action: the file goes, the worker's picture
  comes back), after which every splat as trained is exported. In the 3D view, Export is disabled while its edits wait to
  be discarded there, and while its last save of them failed (the file would bring back what the view hides): Retry in
  Select splats first.
- Splat edits saved again while the file is made: it is not saved (it would not be what the view shows now), and **Try
  again** exports the edits there are now. The edits' generation is read again just before the rename.
- The capture reconstructed again while the file is made: not saved either, and **Try again** exports the new splats. What
  is compared is the identity shares compare (`upload_prep::ResultIdentity`: its report, its PLY's length and time, its
  edits), read before the PLY and again just before the rename: two reconstructions without edits have the same edit
  generation. A result left half replaced (a file of it was held) is no one result: it is not exported until it is
  reconstructed again.
- Not enough room on the disk: the file's size is known before it is written (an SPZ's as what it compresses, at most), and
  it is written only if the disk keeps 64 MB free after it (**Choose another place** offers the same name elsewhere). A disk
  that fills during the write gives the same message.
- A place N4N0 may not write in (a read-only file in the way, a protected folder, a file of that name open in another
  program, a folder gone with its drive), or anywhere inside the capture's own folder (reconstructing it again deletes
  its previous `reconstruction/` folder whole, folders in it included; deleting the capture deletes the rest for good):
  why, and Choose another place.
- Every splat hidden or deleted: said so (nothing to export).
- Anything else (no splats, a damaged PLY): what happened, and Try again.

The PLY is read and the file made a few MB at a time, straight into the part: the splats are the only whole copy of the
capture in memory, and a stop takes effect between two pieces. The file is written beside its name (`<name>.<n>.part`, a new file of its own: never a file already there, nor another
export's; flushed to the disk) and renamed over it, so an export that fails or is stopped never leaves a truncated file
under the chosen name and never a part file; a file that was in the way stays as it was. A dialog closed, or let go of
by its page, stops its export. Only the app quitting mid-write can leave a part file (the chosen name stays as it was). Code: `crates/orbiters-desktop/src/splat_export.rs` (the export), `src/pages/splat_export.rs` (the dialog);
the walkthrough with its screenshots is `--screenshot <out> --sessions <copy> --only splat-export`.

### Why SPZ version 3

Niantic published SPZ 4 (independent Zstandard streams behind a table of contents) on 5 May 2026, and states that old
loaders cannot read it, while every SPZ 4 reader still reads 1 to 3. The app writes version 3 (`export::spz3`). It has
the same quantisation and smallest-three rotations, in the single gzip stream all readers know. It costs 0.45% more than
version 4 on Home (12.89 MB against 12.83 MB for the same 731,087 splats; 1.3 s against 0.9 s to encode in a debug
build). The command line still writes version 4.

### Checked with other readers

Home's three exports (731,087 splats each, from the desktop's own export):
- Niantic's `spz` 1.1.0 (Python) read the SPZ through its legacy path (version 3, antialiased). Positions were within
  0.12 mm of the PLY's, quaternions had a dot of at least 0.999998, opacity was within 1/510, and log scales were within
  1/32 wherever SPZ holds them. 55,657 log scales below −10 saturate, as the reference encoder does: splats thinner than
  45 µm.
- `@adobe/spz` 0.2.3 (the same library as WebAssembly): 731,087 points, antialiased, the same positions.
- PlayCanvas splat-transform 3.10.3 converted the SPZ and the PLY with every splat (PLY positions exact).
- The app's SPZ is byte for byte the encoder's; its GLB holds the same JSON (keys in another order) and binary chunk.
- three.js' `GLTFLoader` with `GLTFGaussianSplatLoaderExtension` loaded the GLB as a `GaussianSplat` of 731,087 splats.
  Its positions are exactly the PLY's with X and Z negated, i.e. glTF's frame.

Not opened yet in the applications themselves: VRChat Gaussian Splatting, SuperSplat, Spark, Cesium, Blender add-ons.

## From the command line

The worker can export an Orbiters reconstruction PLY without loading the GPU:

```
orbiters-reconstruct --export-gltf <splats.ply> <out.glb>
orbiters-reconstruct --export-spz <splats.ply> <out.spz>
```

Both exports contain degree-zero splats only, without what the 3D view hid or deleted. Sky-shell discs marked by Orbiters
and the separate sky panorama are omitted. The input is the binary reference-layout PLY accepted by the worker, not an
arbitrary polygon mesh.

## Coordinates

Each file is written in its own format's frame, so that the capture sits the way it was taken in every reader:

- GLB: glTF's frame (right-handed, +Y up, an asset's front facing +Z), which is Unity's world with **X** negated. A glTF
  importer into Unity negates X back (glTFast's and UnityGLTF's convention), so the capture would land where it was
  captured once such an importer reads the splat extension. Blender's importer puts what the capture looked at in its
  front view.
- SPZ: SPZ's stored frame RUB (+X right, +Y up, +Z back, as three.js), which is Unity's world with **Z** negated, like the PLY and the web `.splat`. The SPZ library's Unity conversion (RUF) negates Z back.
- PLY: RUB, as N4N0's own PLY and SPZ.

The two files therefore differ by half a turn about Y, as the formats' own conventions do (the SPZ library converts RUB to glTF's LUF by negating X and Z). Checked with an asymmetric scene through three.js 0.187's `GLTFGaussianSplatLoaderExtension` (with glTFast's X negation) and Niantic's reader converting to RUF and LUF; the unit tests `glb_comes_back_into_unity_as_captured` and `spz_comes_back_into_unity_as_captured` keep it so.

## Formats

GLB uses the ratified `KHR_gaussian_splatting` ellipse kernel, perspective projection, and camera-distance sorting declaration. Rotations are normalised xyzw, scale is linear, opacity is sigmoid-activated, and colour is SH0 in `srgb_rec709_display`. Blend in the declared space before display conversion. Every value keeps its float range. Different renderers can still differ in sorting, filtering and cutoff; exact image equivalence is not promised. The extension is required (`extensionsRequired`), so a loader without it (Blender's stock importer, Unity's glTF importers today) refuses the file instead of drawing a million uncoloured points; a `COLOR_0` point fallback without the requirement was considered and not made, since nothing here could check that splat readers still prefer SH0 over it.

SPZ writes version 3 from the app and version 4 from the command line, with the same streams. Version 3 is a 16-byte header and the five streams in one gzip stream. Version 4 is a plaintext 32-byte header, a five-entry table of contents for degree zero, and independent Zstandard streams. Positions are signed 24-bit fixed point with 12 fractional bits; positions beyond ±2048 m are rejected (N4N0's worlds reach about 210 m without the sky shell). The standard quantises log scale to 8 bits, opacity and SH0 colour to 8 bits, and xyzw rotation to a 32-bit smallest-three quaternion. Log scales outside [-10, 5.9375] and SH0 colours outside about [-0.44, 1.44] saturate, as in the reference encoder; the CLI prints the number of affected components and the app notes the colours. The header's antialiased flag is set: N4N0 trains with Mip-Splatting's 2D filter and opacity compensation, which readers that honour the flag reproduce. glTF has no such field, so a GLB reader draws faint sub-pixel splats (mostly far detail) stronger than N4N0 does. Neither SPZ is the draft glTF SPZ-v2 compression extension.

PLY writes the reference 3DGS layout alone (`x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3`, floats as trained), without N4N0's own properties (`sky`, `seen_*`), which other tools would carry along unread.

## Colours

Learned colours are not bounded. N4N0's rasteriser blends whatever the optimiser learns, and many splats darken what lies behind them with a negative channel (Home: 31% of non-sky splats, 25% by opacity; Plaza 11%; Needle 17%). `KHR_gaussian_splatting` requires a renderer to clamp each splat's negative colour to zero before blending, and three.js converts colours to clamped RGBA8, so those places render brighter than in N4N0 whatever the file holds. Rendered by N4N0 with the same clamps, held-out PSNR falls by 0.27 dB (Home), 0.36 dB (Plaza) and 0.15 dB (Needle) when negative colours are clamped to zero, and by 0.65, 0.44 and 0.30 dB when colours are clamped to [0, 1] (as three.js does, and as the VRChat tree's and web preview's colour bytes already do). The GLB and the PLY keep the learned floats; clamping them in the file would change nothing in a conforming renderer and lose range for float readers. Matching appearance needs the trainer to learn colours the standard can draw (clamped per splat, as the reference 3DGS rasteriser does).

The `zstd` and `flate2` runtime dependencies and the lab example's `half` dev dependency reuse versions already in Cargo.lock; `serde_json` was already a crate dependency. The independent-reader results and compact tree proposal are in the compact-formats lab report; the deployed tree remains version 1.

The installed generic glTF validator does not yet understand the ratified extension's attribute names; the dedicated extension loader is required for the GLB check.
