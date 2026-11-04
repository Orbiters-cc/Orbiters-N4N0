# 3D pictures from every side (2026-11-04)

Enzo's 4 s picture in ちま部屋 (0.3.4-dev.27, 12-14-49, 318,063 splats, 295 views) looked perfect from where it was taken and
terrible turned to the side: the character and the room smeared. This records what the three pictures taken in that room show,
what changed, and what is measured and what is not. Code: `crates/orbiters-desktop/src/controller/around.rs` (the flight),
`controller/burst.rs` (its driver), `orbiters_core::bundle` (depth records readable while recording).

## What the close-ups really did

From `frames.bin` (each frame's pose and P11) and `parameters.bin` (what VRChat said back):

| | 12-14-49 (4 s) | 12-15-20 (8 s) | 12-16-25 (1 s, no zoom) |
|---|---|---|---|
| Camera's first move after the first frame | 3.17 s | 3.25 s | 0.38 s |
| Zoom first changes | 1.35 s | 1.14 s | never |
| Widest field of view | 89.1° | 89.1° | 47.3° all along |
| Back at 47.3° | 3.42 s | 3.43 s | |
| Keyframes kept in that time, all from the start's place | 5 | 6 | |
| Ends at the start's pose and zoom | yes (P11 within 0.001, turned 0.03°) | yes (P11 within 0.001) | yes |

- **VRChat honoured the zoom, the wrong way.** `/usercamera/Zoom` is a focal length: 45 gives P11 2.2857 (47.3°) and 20 gives
  P11 1.0159 (89.1°), so P11 = Zoom / 19.69. 20 is its widest, not its narrowest. The code took it for a field of view.
- **VRChat does report Zoom** when it changes: 102 messages in 12-14-49. It sends nothing at rest, which is why it looked absent.
- **The camera could not turn while it zoomed.** No `/usercamera/Pose` echo and no change of pose from the first Zoom message
  until 0.6 to 0.9 s after the last (1.29 s to 3.04 s in 12-14-49; the 1 s picture without Zoom moves at once). So neither the nine turned close-ups
  nor the four quick views of the face after them ever happened: every frame of that time is the one place.
- The app sent on time (Zoom 45 at 2.35 s); VRChat's view began to narrow again at 2.88 s, the first pose at 3.04 s.

**Decision: removed**, with every Zoom message. Fixing the sign would still leave a camera that cannot turn while zoomed, at
0.8 s each way: about 2 s for one narrow view. Going closer gives the pixels and the angles together, at once.

## Why the side view was terrible

| Candidate | Evidence | Verdict |
|---|---|---|
| (a) Coverage | Of 37 full-resolution views, 16 show his head with nothing in the way: 15 within 30° of the start, 1 at −34°. Nothing beyond. Rig faces reach +69°, at 38 px/m against 473 | **Yes: the cause** |
| (b) Few steps | ×3 the base steps: +0.7 to +0.9 dB on the body from the side; ×6: +1.5 dB at 3.2 × the time. With one side's views only, ×2 gave +0.26 dB | Real, small, not what he saw |
| (b) Needles | Long axis along the view ray: 0.2 % of his splats (over 4× as long as wide). Share over 4×: 22–23 % with one side's views, 19–21 % with a ring. Seen from the side they are front-facing discs on edge | **No**: no guard built, none tried |
| (c) He moved | Hips within 0.1 mm, head bone 0.00°, chest 0.04° over 172 skeleton frames; Viseme, Voice, velocities constant; desktop | **No** on these |

The flight was never shaped around people. It circled the camera's own place, right when the wearer stands at the camera, but he stood
2.6 m ahead of it, then flew 10 m each way and up, straight through the walls.

| Seconds | 4 s picture (of 7.17) | 8 s picture (of 11.01) |
|---|---|---|
| At the start's place (pause, frozen zoom, turn in place) | 4.00 | 4.11 |
| Elsewhere, his head somewhere in the frame, walls or not | 1.64 | 3.68 |
| Elsewhere, not in the frame | 1.53 | 3.22 |
| Of the last two, outside the room | 1.44 | 4.32 |

## The flight now

After the first frame the camera waits 0.33 s (it was 1.33 s: the face detector now reads while the camera moves), then:

| Part | 4 s | 8 s | What |
|---|---|---|---|
| Look | 0.7 s | 0.7 s | One level turn where it stands: the depth frames say what is free |
| People | 2.2 s | 4.0 s | Whole body at chest height, then head and chest from about 1.4 m, a little above; an 8 s picture a third sweep from higher up. Rig off: every colour frame a whole picture |
| Room | 1.1 s | 3.3 s | Rig on. Hops to the places farthest from where the camera has been, each straight line seen free; through home when there is no other way; home as soon as nothing new is left |

Before: 0 s planned on people. Groups of people within 1.4 m are one subject; up to 3 subjects share the people's time, three in a 4 s
picture get one sweep each.

**Who.** Faces of the start frame, placed by depth through their boxes; the wearer from hips and eye height, within 6 m. A face
whose surroundings all lie at its depth is a poster and is skipped.

**Where.** A place counts as free when a depth frame measured past it and none measured a surface there (a stray sample torn from
a silhouette's edge is outvoted, and a bin's second-nearest sample counts). Unknown is not free. A radius shrinks up to 0.55 ×,
and an arc closes to what is free, there and back; gaps of up to 30° that only a person hid are bridged. In a room too small for the whole
body, the aim swings between head and feet.

**No depth frames** (fewer than 3 after the look), or the camera dolly's pre-planned paths: the old blind flight. The 1 s picture is
unchanged. The 3D view opens on the first frame of any recording of 15 s or less.

### On the real room (tests over the capture's depth and poses)

`chima-room.bin`: the capture's real depth at the start, and voxels of all three pictures' depth. Frames the flight would see from poses never
flown are drawn by casting rays through those voxels, so this checks the logic, not VRChat.

| | Recorded | Planned, 4 s |
|---|---|---|
| Full-resolution views showing his head | 16 | 50 |
| 30° sectors of the circle with such a view | 2 | 10 of 12 |
| Best px/m on the head from a side | none | about 830 |
| Through a wall | yes, out of the building | never |
| Ends at the start pose, zoom untouched | yes | yes |

Feet are framed in at least 7 of 12 sectors, close views in at least 10 of 12: the room around him is tight. Above the head, the 2.7 m
ceiling leaves little room for the third sweep.

## What such views are worth (real views held out)

VRChat Home, 2026-10-21 15-11-55: the only capture found with a ring around a still avatar, az 179° to 59° at 2.8 to 4.3 m, every
hold-out a real recorded frame never trained on. Seven held out; side scores over the five at 18° steps, on the body's box and the whole view.
Three runs each, shared RTX 4080. Dev.27 training.

| Training views | Body PSNR | SSIM | LPIPS | Whole PSNR | SSIM | LPIPS |
|---|---|---|---|---|---|---|
| One side only (19) | 18.47 | 0.760 | 0.392 | 19.16 | 0.824 | 0.279 |
| Every 13° around (24) | 23.47 | 0.912 | 0.150 | 23.53 | 0.924 | 0.106 |
| Every 4° around (35) | 24.07 | 0.929 | 0.119 | 25.01 | 0.940 | 0.085 |

Run-to-run SD under 0.1 dB except where given below. The room gains with the body.

## Training changes tried

On the 24-view ring unless said; side body scores, three runs; control 23.33 dB, LPIPS 0.150, 16.4 s.

| Change | Body PSNR | LPIPS | Whole | s | Decision |
|---|---|---|---|---|---|
| Tiles on whole bodies from every view, no moment | 23.41 | 0.147 | 23.70 | 15.9 | **Removed**, below |
| 8000 tile steps with those | 23.31 | 0.142 | 23.69 | 22.6 | No |
| Nothing held out of training | 23.43 | 0.152 | 23.85 | 15.6 | No effect |
| Base steps × 2 (10,000 in all) | 23.43 ± 0.12 | 0.134 | 23.91 | 27.5 | Not default |
| × 3 (13,000) | 24.09 ± 0.43 | 0.125 | 24.11 | 38.0 | Not default |
| × 6 (18,000) | 24.83 | 0.115 | 24.60 | 52.9 | Not default |

Whole-body tiles took the friends' faces in the first frame down, three runs each:

| Penthouse Palace | Face | Without | With |
|---|---|---|---|
| 8 s | L | 33.83 dB, LPIPS 0.014 | 18.27 dB, 0.456 |
| 8 s | R | 40.03, 0.038 | 30.37, 0.327 |
| 4 s | L | 22.61, 0.205 | 18.71, 0.372 |
| 4 s | R | 37.47, 0.069 | 29.38, 0.249 |
| 8 s, whole held-out | | 26.91 | 28.36 |
| 4 s, whole held-out | | 23.61 | 24.73 |

So the first-moment weighting stays: it costs later moments about 1.1 to 1.5 dB and holds the picture's moment. On ちま部屋 they cost 0.3 to 0.75 dB
held-out, for nothing visible. Steps per view are not raised: a real flight's many views already raise the steps (100 a view).

Only one change remains in training: a picture with more than 80 keyframes still gets its faces read (40 keyframes spread over it), by
its length of at most 15 s. Same 3D picture, same training otherwise.

## Costs

| | Before (dev.27) | Now |
|---|---|---|
| 4 s picture, from first frame to last | 7.2 s | about 4.7 s |
| 8 s picture | 11.0 s | about 8.7 s |
| Standing still before the camera moves | 3.17 s (1.33 s pause, then frozen) | 0.33 s |
| Views for training, 4 s | 37 + 258 rig faces | about 83 + 54 rig faces (simulated) |
| Views for training, 8 s | 57 + 642 rig faces | about 144 + 180 rig faces (simulated) |
| Training, 4 s (no VRChat running) | 17.6 s on this machine (21.6 in the app that day) | **estimated** 33 s, 12,300 steps |
| Training, 8 s | 26.5 s (35.4 in the app) | **estimated** 50 s, 18,400 steps |

The estimates scale measured steps at about 2.7 ms each; no such flight exists to train yet. The detector takes under a second more
(40 keyframes at 60 to 70 ms when more than 40 are kept; 2.3 to 2.5 s measured at 37 to 57).

## Not measured

- **Nothing here flew in VRChat.** The look's depth, the live reading of `frames.bin` under a real capture, the sweeps' pace and framing:
  unit tests read a real depth file written as the capture does, and the simulated flight's frames are drawn from voxels.
- The score of a picture taken with the new flight: no capture exists. The ring above is another world with other spacing.
- Moving people: a friend turning during the 2.2 s sweep is not covered. The wearer stood exactly still in all three of these.
- Other worlds: a large open world's flight was tested only by the wall and open-space unit cases.
- Avatars of odd size: height comes from eye height for the wearer, from depth or a 1.55 m guess for others.
