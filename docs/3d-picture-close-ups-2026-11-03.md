# Sharper people in 3D pictures: close-ups at the start (2026-11-03)

Enzo: "When I take a 4 or 8 sec 3D picture, [make it] take a few more captures with high resolution of where the camera
points at at the starting point, because right now when I try to take a 3D picture with friends, our faces are super
mushy with very poor details."

## Why the faces were mushy

Measured on the 3D pictures Enzo took with friends that night (`Documents/Orbiters/Sessions/Penthouse Palace
2026-11-03 02-19-18`, 4 s, and `02-19-37`, 8 s; 0.3.4-dev.18, 1920 × 1080 Spout):

| Factor | 4 s picture | What it does to the faces |
|---|---|---|
| The people move while the camera flies | The start keyframe and the last one, from the same place (14 cm higher) 4.3 s apart, differ by 15.4 dB PSNR (SSIM 0.62) on the people | A static model learns the average of their poses: ghosted eyes, doubled hair |
| One sharp view of them from the start | 37 colour keyframes; 5 within 20° and 0.5 m of the start pose, at 5 moments over 4.3 s (0.5, 1.8, 2.4, 3.6, 4.8 s); the one closest to it, the end 14 cm above it, was held out (every 8th keyframe never trains); the 0.7 s the camera stood still at the start gave one keyframe (the capture keeps a frame when the camera moved or turned) | Nothing pins the people as they were framed |
| Few steps on that view | 3700 steps (100 a keyframe); a quarter depth, half the colour steps on the rig's faces: about 40 full-resolution steps on the start keyframe | Even the start view is not learnt: rendered at that very keyframe, 21.9 dB and SSIM 0.875 on the people, half the frame's gradient energy |
| The rig's faces | 268 faces of 420 px over 90° (4.7 px a degree against 23 for the keyframes), half the colour steps | Low-resolution views of the people at other moments again |
| The capture's resolution | Faces 250 px across at 1 m in the start frame, about 100 px at 2–3 m (8 s picture) | Not the limit at 1 m: the start frame itself is sharp |

So what dominates is not the stream's resolution but that the reconstruction never sees the people sharply at one
moment from where the picture was framed: they move, and the views of them that train are few, scattered in time and
mostly low resolution. More pixels of them help only if they come together, at the start, and train enough.

## What changed

### The capture (controller/burst.rs)

The 4 s and 8 s pictures start with **close-ups** (`burst::closeups`, `burst::closeup_steps`): once the recording has
settled (0.7 s, as before), VRChat's camera stays where it stands and zooms in (`/usercamera/Zoom`, VRChat 2025.3.3+:
20 to 150, its vertical field of view in degrees, 45 by default) to 2.4 times the start's pixels a degree (no narrower
than 20°: from the default 45°, 2.35 times), then turns to nine points of its own framing, three rows from the top,
each row the other way from the last. The rows overlap by 30 % of a frame's height so that the band of each lower
row's frames that the capture strip covers (6.7–22 % from the top, which training leaves out) is seen clear in the row
above; together the close-ups cover the whole framing below the top row's strip band (a unit test checks the clear
coverage of the start view through the strip's and band's real layouts). Each close-up is held 0.1 s (the first 0.2 s
while VRChat zooms), then the camera zooms back out where it started for 0.1 s and flies as before: 1.1 s more in all.
The rig stays off during the close-ups (every frame a whole picture), and the flight still ends where it began, with
the start's zoom put back. Pose and zoom go in one OSC bundle each step; nothing about the zoom has to be assumed but
which way to turn: every frame's strip carries its exact pose and projection. Without a zoom VRChat reports
(`/usercamera/Zoom` from its messages or its OSCQuery tree) or with the camera zoomed in already (under 1.35 times to gain), there are no
close-ups, since the zoom could not be put back or would add nothing. The 1 s picture is unchanged.

Why zoom rather than VRChat's photos (`/usercamera/Capture`): a photo needs the camera in Photo mode, which stops the
Spout stream the recording reads; its resolution is a setting OSC cannot read or set (1080p by default, so often no
better than the stream); it would carry the capture kit's strip into Enzo's Pictures folder; and it has no pose of its
own. Zoomed frames are ordinary stream frames: exact pose and projection in their strip, depth frames among them (2.4
times the depth samples on the people), the same file format, and they cost nothing outside the recording.

### Keyframes (orbiters-core keyframes.rs, orbiters-capture)

The capture kept a keyframe when the camera moved 0.3 m or turned 15°: the close-ups, from one place, 8–23° apart at 20°
and the first at the start's pose, would mostly have been dropped. A frame is now also a keyframe when the zoom changed
by 20 % since the last keyframe, and turning enough is a third of the view's height when that is less than the
setting's 15° (6.7° at 20°; VRChat's default 45° keeps 15°). The capture passes the strip's P11 (`consider_view`).

### Training (orbiters-reconstruct dataset.rs, train.rs)

Keyframes whose focal length is at least 1.3 times the session's median (and fewer than a third of all) are **detail
views** (`dataset::detail_views`). They are never held out (`train::held_out_views`: every 8th of the others, so a
session without detail views holds out the same keyframes as before) and take a quarter of the colour steps
(`Settings::detail_share`), on top of the steps the other views get without them (`train::detail_iterations`): the other keyframes and
faces keep their steps, the close-ups' come in addition.

### Viewing (desktop views.rs)

A 3D picture with close-ups opens in the 3D view (and the shared web preview) exactly as its camera framed it: the
start keyframe, at the camera's own place, rather than the first held-out keyframe at the photographer's eye height,
which for a 3D picture was a view from the middle of the flight.

### VRChat trees

Nothing to change: a 3D picture's model (350–600k splats) fits the PC tree's 2,097,152 whole, whose re-fit touches only
colours and opacities (the splats' places and shapes, where the detail is, stay); the Quest tree ranks splats by what
they add to the recorded cameras, among which the close-ups count. Measured below.

## Measurements

No VRChat here (it cannot run in the lab), so the close-ups were **emulated** on the real 3D pictures: a lab script
(the SOTA lab's `scratch-hires-faces/mkdetail.py`) adds nine keyframes that are native-resolution crops of the start
frame (3 × 3, overlapping, from below its strip to above its band), each stored upscaled to the frame size with the
crop's exact projection, and the session is trained at half resolution (`--width 960`): every crop then carries twice
the other keyframes' pixels a degree, as a 20° close-up through the 1080p stream carries 2.35 times. The people are
scored at full resolution against the start frame (the picture's moment, which the crops are of) in a box over them;
whole-scene quality on the run's held-out keyframes (every 8th, the same ones in every arm). Honest limits: the crops
show the people at exactly the start frame's moment (real close-ups take a second, the people move a little), the
emulation's magnification is 2 rather than 2.35, no depth frames come with it (real close-ups bring zoomed depth), and
the score's reference is the frame the crops come from. The static plaza walk (no people, ground truth everywhere) is
the control for what the close-ups cost the rest of a scene: crops of keyframe 11, scored on keyframe 12, a held-out
view 0.37 m further on. Runs on one RTX 4080 shared with other jobs; repeats agree within 0.2 dB (seeded).

People in the start view (PSNR dB / SSIM at 1920 × 1080), whole-scene held-out keyframes (PSNR / best 90 % of pixels),
training time at half resolution and splats; mean ± SD, n = 3 unless noted:

| Picture | Arm | People | Held-out | Best 90 % | Time | Splats |
|---|---|---|---|---|---|---|
| 8 s (n = 2) | before | 22.28 / 0.868 | 28.37 ± 0.28 | 35.87 ± 0.07 | 24.6 s | 618k |
| | close-ups as plain keyframes (n = 1) | 31.39 / 0.97 | 28.45 | 35.87 | 28.0 s | 672k |
| | close-ups, 15 % (n = 1) | 34.45 / 0.98 | 28.32 | 35.68 | 27.9 s | 669k |
| | **close-ups, 25 %** (the default; n = 3 with the default run) | **36.21 ± 0.11 / 0.99** | 27.99 ± 0.33 | 35.17 ± 0.46 | 33.1 s | 763k |
| 4 s | before | 22.06 ± 0.11 / 0.87 | 25.26 ± 0.27 | 33.74 ± 0.82 | 10.6 s | 410k |
| | close-ups as plain keyframes | 26.71 ± 0.19 / 0.94 | 24.08 ± 0.08 | 33.63 ± 0.18 | 12.9 s | 350k |
| | close-ups, 15 % (n = 1) | 26.33 / 0.94 | 23.87 | 32.78 | 13.1 s | 324k |
| | **close-ups, 25 %** | **27.73 ± 0.04 / 0.96** | 23.29 ± 0.10 | 32.86 ± 0.56 | 14.4 s | 516k |
| | close-ups, 40 % (n = 2) | 28.57 ± 0.09 / 0.97 | 22.99 ± 0.23 | 31.62 ± 0.26 | 19.1 s | 555k |

The 4 s picture's held-out loss is its views of the people at other moments: one keyframe (34, a pass close over a
friend's body 0.7 s after the start) goes from 27.3 to 23.5 dB on its best 90 %, the four others are within ±0.5 dB, and
the per-pixel difference maps show the change on the people only (the SOTA lab's `reports/hires-faces/`). The model
now holds them as they were when the picture was taken, which views of them a second later disagree with.

Static plaza (n = 3; 15 %, n = 1):

| Arm | Start view (trained) | Held-out view 0.37 m on | Other held-out | Best 90 % | Time |
|---|---|---|---|---|---|
| before | 34.00 | 32.96 | 36.08 ± 0.04 | 42.18 | 44.6 s |
| close-ups, 15 % | 37.14 | 34.17 | 35.59 | 41.37 | 50.1 s |
| close-ups, 25 % | 38.19 ± 0.10 | 33.76 ± 0.05 | 35.73 ± 0.04 | 41.69 | 57.2 s |

So a static scene loses about 0.4 dB away from the picture's framing for 0.8 dB on a new view near it and 4 dB on the
framing itself: the trade a picture wants. A quarter of the colour steps is the default (`train::DETAIL_SHARE`): of the
shares tried, it gives the most on the people without the cost elsewhere that 40 % has.

What reaches VRChat (the upload's export, `orbiters-reconstruct --vrchat`, its leaves rendered at the start view;
n = 1):

| Picture | Arm | Model | PC tree | Quest tree | PC file |
|---|---|---|---|---|---|
| 8 s | before | 22.28 / 0.868 | 22.35 / 0.864 | 22.27 / 0.863 | 15.9 MB |
| | close-ups | 36.09 / 0.987 | 33.60 / 0.974 | 33.53 / 0.970 | 19.8 MB |
| 4 s | before | 21.97 / 0.873 | 22.08 / 0.869 | 22.00 / 0.869 | 10.7 MB |
| | close-ups | 27.77 / 0.963 | 27.32 / 0.954 | 27.40 / 0.953 | 13.4 MB |

The trees keep almost all of it; the PC tree's 2.5 dB under the model on the 8 s picture's faces is the file's rounding
(32.6 dB without the leaves' re-fit, which gains 1 dB back from cameras 960 wide): a lead for the tree format, not a
cap this change could lift. The Quest file stays at its cap (10.3 MB).

The 3D view: `screenshot --only splats-3d` on the emulated 4 s picture now opens on the people as framed (before: a
view of the pool from the middle of the flight).

## What only VRChat can tell

Whether `/usercamera/Zoom` takes effect over OSC in Stream mode at once (the close-ups are useful zoomed or not: each
frame's strip says what it was), that `/usercamera/Zoom` is the vertical field of view (the tiles' coverage assumes it;
the 1080p frames of 2026-11-03 had 47.3°, the default being 45), and how sharp real close-ups make faces. See the lab
report's test plan.
