# Export a reconstruction

The worker can export an Orbiters reconstruction PLY without loading the GPU:

```
orbiters-reconstruct --export-gltf <splats.ply> <out.glb>
orbiters-reconstruct --export-spz <splats.ply> <out.spz>
```

Both exports contain degree-zero splats only. Sky-shell discs marked by Orbiters and the separate sky panorama are omitted. The input is the binary reference-layout PLY accepted by the worker, not an arbitrary polygon mesh.

## Coordinates

Each file is written in its own format's frame, so that the capture sits the way it was taken in every reader:

- GLB: glTF's frame (right-handed, +Y up, an asset's front facing +Z), which is Unity's world with **X** negated. Unity's glTF importers (glTFast, UnityGLTF) negate X back, so a GLB imported into a Unity world lands where it was captured; Blender's importer puts what the capture looked at in its front view.
- SPZ: SPZ's stored frame RUB (+X right, +Y up, +Z back, as three.js), which is Unity's world with **Z** negated, like the PLY and the web `.splat`. The SPZ library's Unity conversion (RUF) negates Z back.

The two files therefore differ by half a turn about Y, as the formats' own conventions do (the SPZ library converts RUB to glTF's LUF by negating X and Z). Checked with an asymmetric scene through three.js 0.187's `GLTFGaussianSplatLoaderExtension` (with glTFast's X negation) and Niantic's reader converting to RUF and LUF; the unit tests `glb_comes_back_into_unity_as_captured` and `spz_comes_back_into_unity_as_captured` keep it so.

## Formats

GLB uses the ratified `KHR_gaussian_splatting` ellipse kernel, perspective projection, and camera-distance sorting declaration. Rotations are normalised xyzw, scale is linear, opacity is sigmoid-activated, and colour is SH0 in `srgb_rec709_display`. Blend in the declared space before display conversion. Every value keeps its float range. Different renderers can still differ in sorting, filtering and cutoff; exact image equivalence is not promised.

SPZ writes version 4: a plaintext 32-byte header, a five-entry table of contents for degree zero, and independent Zstandard streams. Positions are signed 24-bit fixed point with 12 fractional bits; positions beyond ±2048 m are rejected (N4N0's worlds reach about 210 m without the sky shell). The standard quantises log scale to 8 bits, opacity and SH0 colour to 8 bits, and xyzw rotation to a 32-bit smallest-three quaternion. Log scales outside [-10, 5.9375] and SH0 colours outside about [-0.44, 1.44] saturate, as in the reference encoder; the CLI prints the number of affected components. The header's antialiased flag is set: N4N0 trains with Mip-Splatting's 2D filter and opacity compensation, which readers that honour the flag reproduce. glTF has no such field, so a GLB reader draws faint sub-pixel splats (mostly far detail) stronger than N4N0 does. SPZ v4 requires an updated reader; it is not the draft glTF SPZ-v2 compression extension.

## Colours

Learned colours are not bounded. N4N0's rasteriser blends whatever the optimiser learns, and many splats darken what lies behind them with a negative channel (Home: 31% of non-sky splats, 25% by opacity; Plaza 11%; Needle 17%). `KHR_gaussian_splatting` requires a renderer to clamp each splat's negative colour to zero before blending, and three.js converts colours to clamped RGBA8, so those places render brighter than in N4N0 whatever the file holds. Rendered by N4N0 with the same clamps, held-out PSNR falls by 0.27 dB (Home), 0.36 dB (Plaza) and 0.15 dB (Needle) when negative colours are clamped to zero, and by 0.65, 0.44 and 0.30 dB when colours are clamped to [0, 1] (as three.js does, and as the VRChat tree's and web preview's colour bytes already do). The GLB keeps the learned floats; clamping them in the file would change nothing in a conforming renderer and lose range for float readers. Matching appearance needs the trainer to learn colours the standard can draw (clamped per splat, as the reference 3DGS rasteriser does).

No desktop export UI is introduced. The added `zstd` runtime dependency and the lab example's `half` dev dependency reuse versions already in Cargo.lock; `serde_json` was already a crate dependency. The independent-reader results and compact tree proposal are in the compact-formats lab report; the deployed tree remains version 1.

The installed generic glTF validator does not yet understand the ratified extension's attribute names; the dedicated extension loader is required for the GLB check.
