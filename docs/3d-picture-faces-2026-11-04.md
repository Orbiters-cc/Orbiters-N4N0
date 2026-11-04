# Faces in 3D pictures: why the mouth was mushy, and what changed (2026-11-04)

Enzo, on a 3D picture of himself (METAFES 2026 AUTUMN, 2026-11-03 22-58-11, 0.3.4-dev.23): "The eyes are better but for
some reasons the mouth is all mushy. Maybe we can have a quick face detection mechanism and do extra processing on the
face areas even if it cost us a few more seconds, since those are the areas people will focus on the most." Then:
"maybe we can also use this face detection model during the capture to take a few more pictures / angles of the faces in
the scene if that could help".

## What was found on his capture

**The close-ups were never taken.** Every one of its 748 frames has the same field of view (47.27° vertically, P11
2.286), and the camera starts flying right after the settle. `/usercamera/Zoom`, which the close-ups need to put the zoom
back, is not among the 37 `/usercamera/` addresses VRChat 2026.4.1p1 gave that session (OSC or OSCQuery), though its
documentation lists it and every other slider is there. No zoom known: no close-ups. So "the eyes are better" did not come
from them. The picture's framing also held no face: he stood 38 cm behind the camera, which saw him only as it flew.

**The mouth did not move.** He was on desktop, `Viseme` and `Voice` stayed at 0, and the skeleton band's hips and head bone
did not change at all in 72 frames.

**The cause: too few steps on too few views, and a low-contrast mouth.** Two colour keyframes (of 47) see the face closely,
at 38 cm, each with about 20 full-resolution steps of the 4700. Over so few, what training keeps follows the texture's
contrast. Gradient energy against the truth in those views' own renders:

| Region | Forehead | Cheek | Mouth | Nose | Eyes |
|---|---|---|---|---|---|
| Kept | 0.14 | 0.20 | 0.33 | 0.48 | 0.63 |

(About 20: 4700 steps, a quarter on depth, half the rest on the rig's faces, shared by 41 keyframes, half of them at half
resolution. Computed, not counted.) Not the cause: three times the steps spread over all the frames (mouth still 0.35; 35 s
more); a mask throwing pixels away (the robust mask is off by default); the tree on this picture (it keeps what the model has,
below). A cause that remains: the muzzle's geometry from so few angles. With the second close view held out, the face in it
is 19.9 dB, against 23.5 in the view that trained; see "More angles".

## What changed

### Extra training on faces (orbiters-reconstruct `portraits.rs`, `train.rs`, `main.rs`)

- **A detector** (FacesV1, YOLOv8m one class) runs in the worker through the ONNX Runtime the voice worker already ships
  (`ort`, `load-dynamic`). 60–75 ms a keyframe on the CPU; only on sessions of at most 80 keyframes (a 3D picture has 15 to
  70), so a long recording spends nothing. Missing files: it says so on the log and trains as before.
- **What two views agree on**: boxes whose rays meet near where their sizes put them are one head; a lone box is dropped.
  That removes one-off mistakes and the watermark a world pins to the screen. A poster's face seen twice stays: it only
  wastes some tiles.
- **Tile steps**: 400 per view of a face, up to 4000, on top of the others and all in the second half of training, each on a
  512-pixel tile of a keyframe at full resolution, weighted by the box's size. The first half stays the base run's, and
  relocation ends where it would. From the start of training the same steps melted the face (his eyes from 27 to 17 dB in
  their own view; starting at a quarter was also worse). A tile's pixel weighs what a frame's pixel weighs.
- **The moment**: a later view of a person counts e times less each second. People move while the camera flies, and the
  picture is of the moment it was framed. A wearer whose skeleton band says they did not move keeps all their views.
- **No faces**: no tile steps and no extra random draws, so the same steps as ever. Training is not deterministic on this
  GPU (two runs with the same settings differ in every file), so "byte-identical" has no meaning here; the 10-keyframe test
  pattern's two arms have the same 3000 steps, 23.40 and 23.39 dB, and the detector costs 0.9 s.

### The capture (desktop `controller/burst.rs`)

Before a 4 s or 8 s picture flies, the worker reads the frame the recording started on (`orbiters-reconstruct
--find-faces`, 0.6 s, its own process: the app itself carries no detector), and:

- **The zoom** comes from the frame's own projection when VRChat does not report it, so the close-ups can run and the zoom
  can be put back. Whether VRChat obeys `/usercamera/Zoom` in this build is still unknown (below).
- **The camera goes to the faces**: up to three, largest first, plus the wearer's when within 3 m (from the band's hips and
  the eye height). Four views of each at about 0.7 m: straight, 40° each way and from 25° above, held 0.1 s each, with the
  rig off. 0.4 s a face, no zoom needed. Without a face or the worker: no detour.

## Measurements

Real captures, one RTX 4080, shared with other jobs (they waited on one GPU lock). Mean of 3 runs (± SD where it matters);
SSIM; LPIPS (AlexNet, lower is better). Trained views: the picture's own views of a face; held out: never trained.

| Picture | Face | Before | After |
|---|---|---|---|
| 8 s, friends | L, first frame | 17.23 dB / 0.481 / 0.493 | **30.93 / 0.954 / 0.027** |
| | R, first frame | 28.89 / 0.776 / 0.358 | **38.29 / 0.955 / 0.050** |
| 4 s, friends | L, first frame | 17.84 / 0.704 / 0.438 | **23.22 / 0.858 / 0.202** |
| | R, first frame | 28.76 / 0.815 / 0.362 | **37.27 / 0.957 / 0.066** |
| 8 s, Enzo | close, whole face | 23.19 / 0.887 / 0.347 | **25.30 / 0.925 / 0.228** |
| | close, nose and mouth | 28.11 / 0.959 / 0.278 | 29.17 / 0.966 / 0.205 |
| | close 2, nose and mouth | 26.47 / 0.934 / 0.366 | **29.12 ± 1.23 / 0.958 / 0.224** |
| | profile | 25.40 / 0.849 / 0.336 | **29.32 / 0.931 / 0.166** |
| | held out, 1.4 m | 23.14 ± 0.26 / 0.750 / 0.381 | 23.72 ± 0.63 / 0.746 / 0.302 |

What it costs: the same people at other moments, which they no longer match. 4 s friend L at 1.9 s: 23.81 → 20.07 dB; 8 s
friend R at 1.9 s: 30.34 → 28.73 (LPIPS better, 0.363 → 0.174).

| Picture | Held-out, whole picture (PSNR / best 90 % / SSIM) | Whole run | Model |
|---|---|---|---|
| 8 s, Enzo | 21.82 / 31.77 / 0.779 → 21.69 / 31.10 / 0.776 | 18.6 → 28.4 s (+9.8) | 54.7 → 52.6 MB |
| 4 s, friends | 24.94 / 34.14 / 0.829 → 23.38 / 32.17 / 0.801 | 12.3 → 20.6 s (+8.3) | 38.5 → 35.4 MB |
| 8 s, friends | 27.96 / 35.77 / 0.873 → 27.83 / 35.36 / 0.861 | 25.3 → 37.8 s (+12.5) | 54.4 → 46.9 MB |

The 4 s picture's loss is its people at other moments, as with the close-ups (`docs/3d-picture-close-ups-2026-11-03.md`).

**What reaches VRChat** (`--vrchat`, one run, leaves rendered at the view):

| Face | Model | PC tree | Quest tree |
|---|---|---|---|
| 8 s friend L: before | 17.23 | 17.27 | 17.22 |
| after | 30.92 | 24.89 | 26.38 |
| 8 s friend R: before | 29.14 | 29.19 | 29.03 |
| after | 38.37 | 36.22 | 35.50 |
| Enzo, close 2, whole mouth box: before | 26.91 | 26.46 | 26.11 |
| after | 28.55 | 28.26 | 28.08 |

Files: PC 15.7 → 15.0 MB, 15.1 → 13.4 MB, 10.9 → 10.1 MB; Quest at its cap. No export budget was added: the PC tree keeps
every splat, so what it loses is the file's rounding and colour bytes (a lead for the format), and 6 dB of friend L's gain
goes there.

### More angles: simulated

No VRChat in the lab, so the arcs' value was simulated: the best model we could train of Enzo's capture (M) stands in for the
world. Every colour keyframe is M's render at its pose; one arm adds the four views at 0.71 m around his head. Scored against
M from 8 other places around his face at 0.9–1.2 m (2000 tile steps, all views alike, the real depth frames, none for the
arcs):

| Arm | PSNR | SSIM | LPIPS |
|---|---|---|---|
| The flight as it is | 23.83 | 0.822 | 0.286 |
| With the four views | **25.94** | **0.860** | **0.221** |

Every one of the eight gains (0.8 to 3.3 dB). A simulation of a still head: it says what the angles are worth, not what a
real flight will give.

### With zoomed close-ups (emulated, native crops of the first frame, 960 wide, 1 to 2 runs; 2000 tile steps, moment on)

The close-ups and the tiles add up: 4 s friends 31.27 → 33.81 and 41.47 → 44.02 dB; 8 s 37.08 → 39.43 and 43.95 → 46.99 dB.
LPIPS halves or better.

### The detector

Faces at least about 100 pixels wide, counted by eye on the sheets of its boxes, so roughly (`--find-faces`, confidence ≥ 0.4), from the front
or three-quarters: Enzo's own 8 of 9 (the miss a three-quarter view at 1.4 m), 4 s friends 13 of 15, 8 s friends 10 of 16
(in the dark, and side-on): **31 of 40, 78 %**. Its boxes cover the muzzle and mouth in all the close views checked, and a
15 % margin takes the chin. It also finds posters' faces, up to 0.84.

| Export | File | Time a frame (20 threads) | Boxes of the float model (≥ 0.5) kept |
|---|---|---|---|
| Float, 640 × 640 | 103.6 MB | 110–140 ms | |
| Float, 640 × 384 | 103.6 MB | 70–80 ms | |
| **Int8, convolutions only, calibrated on real frames, 640 × 384** | **26.5 MB** (21.8 MB compressed) | **60–70 ms** | **38 of 38, none new** |

Quantizing everything breaks it: the output's boxes and confidences share one range, and the confidence becomes 0.

| Runtime | Added | Load | Time a frame |
|---|---|---|---|
| **`ort` on the shipped `onnxruntime.dll` 1.28.2** | 0.3 MB of code | about 0.3–0.9 s | 63–76 ms |
| `tract` 0.21 | 16 MB | not measured apart | about 1050 ms (float) |

wgpu was not tried: the runtime is already there.

## How to try it

Take a 4 s or 8 s 3D picture with a friend or yourself in it, then open it. `--portrait-steps 0` turns the extra training off
in the worker.

## What only VRChat can tell

- Whether `/usercamera/Zoom` works in Stream mode on this build. If not, the nine close-ups run unzoomed: a second lost, no
  harm. The log and each frame's strip say which.
- Whether the slider and the field of view are proportional: the zoom is put back from one measured point (45 ↔ 47.27°).
- Whether the arcs frame the faces as planned and how they look, in the wearer's case from an estimated head place.
- Faces in VR, with visemes and full face tracking: this capture was on desktop, its mouth still. A mouth moving during the
  picture is what the moment is for, but it was not measured.
