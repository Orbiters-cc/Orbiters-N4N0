# More detail on PC, the ground at the right height, sharing at once (2026-10-29)

Workstream M2 of [Make N4N0 accessible](n4n0-accessible.md). Enzo's three asks:

1. "I would like the 3D captures to have much more details at least on PC. On Quest right now it's perfect, but on PC
   it feels like we're way too limited for the capacity of our machine. [...] even taking over a minute of recording
   doesn't fix it."
2. "When displaying 3D reconstructions, sometimes the POV is too low and sometimes it's too high. We need a more
   reliable way to have the ground at the same level as the ground the player was on when he took the capture."
3. The first upload preparation still took 5 to 25 s ([upload-prep-speed-2026-10-29.md](upload-prep-speed-2026-10-29.md)):
   sharing should feel instant without taking VRChat's frames.

## 1. Where detail stopped

Every cap between a recording and a PC viewer, before this change:

| Stage | Cap | What it did |
|---|---|---|
| Training: fading unseen Gaussians (`train::Settings::fade_unseen_every`) | every 1000 steps until half the run | On long recordings the unseen share snowballed (Silent Dawn, 78 s: 27 %, 41 %, 54 % … 77 % of the set each round): relocation's own copies faded again round after round, the set never grew (growth waits for ≤ 5 % dead), and the result kept 359k of its 888k Gaussians |
| Training: budget | 1,000,000 Gaussians, seeds two thirds of it | A 3D picture of 313 views reached it |
| VRChat PC tree | 1,048,576 splats with merged boxes: about 786k leaves | Reconstructions over that lost their least visible splats |
| VRChat PC world | 32,768 points × 32 = 1,048,576 splats | A bigger tree is refused ("more than this device draws") |
| Web preview | 600,000 splats (19 MB), server 24 MB | The website showed at most 600k of any reconstruction |
| Upload | Cloudflare's 100 MB a request in front of Orbiters | web 24 + PC 36 + Quest 18 + coarse 2 + sky 16 = 96 MB: no room to grow any file |

### Measurements

Lab runs on hard-linked copies of real sessions (`C:/Users/Enzo/orbiters-lab-m2`, never the sessions' own results),
scored by `examples/eval` on the run's held-out keyframes (every 8th): whole-frame PSNR, PSNR of the best 90 % of
pixels (trimmed: transients such as passing avatars and slideshows leave it) and SSIM. One RTX 4080; some runs shared
the GPU with another run (their times are not comparable; the quality numbers are). Seeds are fixed but the float
atomics are not: identical runs differ by up to about 0.4 dB trimmed on these sessions.

| Session (views = keyframes + rig faces) | Worker | Budget | Splats kept | PSNR | Trimmed | SSIM |
|---|---|---|---:|---:|---:|---:|
| Silent Dawn 10-28, 78 s recording (353 + 66) | main | 1M | 358,593 | 23.93 | 31.81 | 0.808 |
| | main | 2M | 429,224 | 25.05 | 31.88 | 0.807 |
| | no fading | 1M | 765,059 | 24.14 | 32.83 | 0.827 |
| | no fading | 2M | 1,336,197 | 25.25 | 33.67 | 0.840 |
| | unseen weight 0.002 | 1M | 768,883 | 24.04 | 32.88 | 0.827 |
| | fading stops when rising | 1M | 758,931 | 23.89 | 32.81 | 0.826 |
| | fading stops when rising | 2M | 1,233,311 | 25.40 | 33.48 | 0.837 |
| | fading stops when rising | 3M | 1,405,011 | 26.21 | 33.83 | 0.843 |
| VRC NYE 10-28 23-47, 9 s 3D picture (103 + 210) | main | 1M | 841,359 | 27.82 | 35.45 | 0.899 |
| | main | 2M | 948,290 | 28.35 | 36.33 | 0.907 |
| | main | 3M | 980,040 | 28.61 | 36.49 | 0.909 |
| | main, 60,000 steps | 2M | 1,546,566 | 28.28 | 36.54 | 0.912 |
| | no fading | 1M / 2M | 912,207 / 1,777,707 | 27.73 / 28.53 | 35.70 / 36.19 | 0.897 / 0.910 |
| VRChat Home 10-20, a walk (137) | main | 1M | 740,434 | 26.22 | 42.93 | 0.943 |
| | no fading | 1M | 818,162 | 25.06 | 42.35 | 0.938 |
| | fading stops when rising | 1M / 2M / 3M | 740,291 / 736,934 / 738,314 | 26.24 / 26.06 / 25.98 | 42.97 / 42.84 / 42.75 | 0.944 / 0.942 / 0.942 |
| Autumn fragrance 10-28 (75 + 164) | main | 1M | 548,707 | 19.22 | 24.36 | 0.639 |
| | fading stops when rising | 1M / 2M | 872,937 / 1,009,647 | 19.58 / 19.85 | 24.93 / 25.14 | 0.657 / 0.668 |
| Find the Needle 10-27 (29 + 156) | main | 1M | 373,201 | 22.86 | 32.16 | 0.871 |
| | fading stops when rising | 1M / 2M | 373,976 / 373,681 | 22.85 / 23.82 | 31.90 / 32.53 | 0.871 / 0.880 |

("Fading stops when rising" in this table is the first form of the rule: a share rising by more than 5 points.)

Readings:

- Detail did not stop at the budget on the long recording: fading ate it. Without fading, Silent Dawn kept twice the
  splats and gained 1.0 dB trimmed; a lower unseen weight did the same. On VRChat Home, fading still helps (0.6 dB), as
  the round that introduced it measured. What tells them apart is the unseen count from round to round: Home's holds
  (356k, 357k, 348k, 335k, 320k of 834k), Silent Dawn's climbs in a set of one size (244k, 368k, 483k, … 680k of 888k),
  and a 3D picture's grows while its set grows (157k of 680k, 278k of 868k, 320k of 1.0M). The trainer now stops
  fading for the rest of a run once a round finds more unseen Gaussians than the round before, by over a twentieth of
  the set, while the set did not grow (`train::Settings::fade_until_rising`; `--fade-to-the-end` restores the old
  behaviour). A first form (a share rising by 5 points) also stopped the 3D picture, which lost 0.25 to 0.45 dB at the
  same budget; a second (any rise) stopped Home at 356,752 against 355,861 and lost 0.3 dB.
- Past that, more Gaussians help sessions with many views: two million gained 0.6 to 0.9 dB on the 3D picture and on
  the recording, three a further 0.35 dB on the recording only, and nothing on Home's 137 views. 60,000 steps instead
  of 10,300 gained 0.2 dB for six times the time: steps stay at 100 a keyframe.
- The budget follows the views: `train::budget_for` gives 7,000 Gaussians a view, at least 1M and at most 3M (Home 1M,
  the 3D picture 2.19M, the recording 2.93M), and the worker always fits it to the graphics memory free (before, only
  beside VRChat), leaving 1 GB.

The final worker (this rule, budget by views) against main, each run alone on the GPU (main's Silent Dawn run back to
back with the final one; the other main runs from earlier the same morning):

| Session | Budget | Main: trimmed dB, splats, training s | Final: trimmed dB, splats, training s | Change |
|---|---:|---|---|---:|
| Silent Dawn 10-28 (419 views) | 2,933,000 | 31.72, 353,622, 117.7 | 33.53, 1,399,065, 262.1 | +1.81 dB |
| VRC NYE 10-28 23-47 (313) | 2,191,000 | 35.45, 841,359, 58.6 | 36.28, 960,566, 44.7 | +0.83 dB |
| Autumn fragrance 10-28 (239) | 1,673,000 | 24.36, 548,707, 25.7 | 25.17, 694,474, 27.2 | +0.81 dB |
| Find the Needle 10-27 (185) | 1,295,000 | 32.16, 373,201, 6.8 | 32.19, 376,338, 6.9 | +0.03 dB |
| VRChat Home 10-20 (137) | 1,000,000 | 42.93, 740,434, 53.1 | 42.87, 740,942, 54.9 | −0.06 dB |

A second final run of Silent Dawn (the first form of the rule, which behaves the same there) scored 33.39 dB with
1,385,664 splats in 253.9 s. Quest is untouched by this: its file still holds at most 524,288 splats, now chosen from
the larger reconstruction.

### What changed for PC (and not for Quest)

- Reconstructions: fading rule and budget above.
- VRChat PC tree: up to 2,097,152 splats (`tree_tune::PC_SPLATS`, contracts' Docs/splat-tree.md, server
  `mapTrees.js`). Quest's tree (524,288), its coarse top and the Quest world are unchanged; Quest's file is chosen
  from a better reconstruction by the same rule as before.
- The world (Orbiters 3D pictures viewer) draws trees of up to 2,097,152 splats on PC: its point mesh has 65,536 points
  (the renderer now reads its capacity from the mesh). Its sort covers the next power of two of a tree's splats, so a
  tree over 1,048,576 sorts twice the entries: measured in the editor on the RTX 4080 (beside a training run), a whole
  sort took 4.5 ms over 1 Mi entries and 5.7 ms over 2 Mi (110 and 121 passes, 0.041 and 0.047 ms a pass, about 1.2
  and 1.4 ms a frame at the PC setting of 30 passes a frame). Both fit in the 4080's 64 MB L2 cache; a GPU with little
  cache and 256 GB/s (GTX 1070 class) should take about 0.1 and 0.2 ms a pass. Not measured in VRChat.
- The upload carries no web file any more: with a 64 MB PC tree there is no room under Cloudflare's 100 MB for a second
  copy of the same splats. The server makes the website's `.splat` from the PC tree's leaves (`mapTrees.js webSplat`):
  all of them (up to the PC tree's), the 600,000 most visible first, each tier from the start view outward. Phones and
  devices short of memory read only the first tier (`frontend/src/components/maps/splatTiers.js`), which is the old web
  preview. Sky maps are at most 8 MB (the desktop writes 2048 × 1024 PNGs of 0.1 to 3 MB); an upload is at most
  66 + 17 + 2 + 8 = 93 MB.

## 2. The ground

Before: the website and the desktop's 3D view opened at the recorded camera (VRChat's stream camera, which N4N0 flies
around the avatar: Silent Dawn's start keyframe was 0.84 m over the floor), and the world kept the capture's own heights
(its anchor stands at y = 0, so a world whose floor is at −4.55 m put players 4.55 m over it, one at +6.37 m put them
under it).

Now `orbiters_reconstruct::ground` finds, for the start view's moment:

- the person: the avatar's hips in the capture kit's skeleton band (`skeleton.bin`), else the camera;
- the ground: the most common height of the flat (thinnest axis under a third of the middle one), level (normal within
  25° of vertical), opaque splats from 5 cm to 3 m under the hips (20 cm to 3 m under a camera), in the smallest of 0.8,
  1.5, 3 and 6 m discs around them holding 24 such splats (the avatar's own reconstructed body is upright and rarely
  level; a sitting avatar's hips stand a quarter metre up);
- the eye height: VRChat's `EyeHeightAsMeters` (else `/avatar/eyeheight`) in `parameters.bin` at that moment, else 1.6 m.

The start view's metadata carries `floor` and `eyeHeight`, and its `position` stands at `floor + eyeHeight` over the
camera's place, looking the same way (contracts' Docs/capture-sharing.md, Start view). The desktop's 3D view opens there
(double-click still goes to the recorded camera), the website opens there, and the world puts `floor` on its own ground.

Every reconstructed session on this PC (`cargo run --release --example ground -- <sessions>`):

| Session | Found from | Floor (m) | Eye height | Hips over it | Camera over it | Eye moves |
|---|---|---:|---:|---:|---:|---:|
| Autumn fragrance 10-28 | hips | −0.752 | 1.66 | 1.01 | 1.02 | +0.64 |
| Exoplanet Journey 10-26 | hips | −4.552 | 1.66 | 0.71 | 1.19 | +0.47 |
| Find the Needle 10-27 | hips | 0.106 | 1.66 | 1.00 | 1.96 | −0.30 |
| Silent Dawn 10-28 | hips | −3.330 | 1.66 | 0.21 (sitting) | 0.84 | +0.82 |
| VRC NYE 10-28 23-47 | hips | −0.168 | 1.66 | 1.13 | 1.85 | −0.19 |
| VRC NYE 10-28 23-49 | hips | 6.371 | 1.66 | 1.00 | 1.71 | −0.05 |
| Canine 10-20 17-06 | camera | 0.788 | 1.60 | | 0.48 | +1.12 |
| Exoplanet Journey 10-20, 10-22, 10-25 | camera | −4.02 to −4.08 | 1.60 | | 1.51 to 1.79 | −0.19 to +0.09 |
| New Generation 10-21 | camera | −0.346 | 1.60 | | 2.24 | −0.64 |
| Palm Point - Night ×4 | camera | 0.21 to 0.59 | 1.60 | | 1.62 to 2.09 | −0.49 to −0.02 |
| Silent Dawn 10-22 ×2 | camera | −3.39 | 1.60 | | 1.55 to 1.68 | −0.08 to +0.05 |
| VRChat Home ×2 | camera | −0.12, −0.03 | 1.60 | | 1.59, 1.66 | +0.01, −0.06 |
| ゆったりHouse 10-22 | camera | 0.841 | 1.60 | | 1.00 | +0.60 |
| Canine 10-20 14-27 | guess (no level splats under the camera) | −0.333 | 1.60 | | 1.60 | 0 |

The world's alignment was checked in the editor on the viewer's own code: a capture whose ground is at −4.33 m and whose
start eye stands 1.66 m over it lands with its eye at (0, 1.66, −2.558) and its ground at the anchor's height (0).

## 3. Sharing at once

When a reconstruction finishes, its session waits for its upload files to be prepared in the background
(`upload_prep::queue`), into the content-hashed cache both share paths read (`upload_prep.rs`): one
`orbiters-desktop --prepare-upload <session>` process at a time, paced as the reconstruction worker (its GPU queue at
Vulkan's low global priority, its process class following VRChat through `orbiters_reconstruct::priority::govern`, and
while VRChat runs in VR every tree depth joins SteamVR's frame-timing governor: `Gpu::follow_vrchat`). The service
starts it only when nothing records or reconstructs (after the reconstruction queue empties), holds it while VRChat
waits under "After VRChat", and never starts it beside VRChat without SteamVR; a capture, a reconstruction, VRChat
started without SteamVR or the headset back on under "After VRChat" stop it and its session waits first in line. A
share of a session waits for that session's own preparation (at most 2 minutes, then it stops it and prepares at once),
and takes the session out of the queue if it had not started.

Measured on two hard-linked session copies (RTX 4080, nothing else on the GPU, no VRChat or SteamVR here):

| Session | Splats | Preparation (background, low priority) | Preparation at share time (before: normal priority) | Share after the background run |
|---|---:|---:|---:|---:|
| New Generation 10-21 | 970,138 | 28.7 s (PC tune 16.0 s, Quest 10.2 s) | 28.9 s | 0.124 s (cache hit, no GPU) |
| VRC NYE 10-28 23-49 | 974,474 | 25.1 s | 24.8 s (PC tune 14.5 s, Quest 7.9 s) | 0.125 s |

(Both take longer than this morning's 23.4 s for New Generation: its PC tree now holds all 970k splats instead of
786k.)

What the preparation does to another program's frames, measured with `examples/framegame` (a stand-in game: Silent
Dawn's 358k splats drawn at 1920 × 1080 every 11.1 ms on a normal-priority device, each frame timed until the GPU
finished it; late = over 11.1 ms):

| Beside | Frames | Late | p50 | p95 | p99 | Worst |
|---|---:|---:|---:|---:|---:|---:|
| Nothing (New Generation's slot) | 1,800 | 2 (0.1 %) | 6.05 ms | 8.70 ms | 9.68 ms | 12.3 ms |
| Background preparation of New Generation | 3,778 | 646 (17.1 %) | 4.47 ms | 17.18 ms | 21.91 ms | 39.4 ms |
| Share-time preparation of New Generation | 3,761 | 639 (17.0 %) | 5.27 ms | 17.44 ms | 22.67 ms | 45.1 ms |
| Nothing (NYE's slot) | 1,800 | 3 (0.2 %) | 6.05 ms | 8.60 ms | 9.83 ms | 12.4 ms |
| Background preparation of NYE | 3,853 | 532 (13.8 %) | 5.66 ms | 15.37 ms | 20.02 ms | 37.3 ms |
| Share-time preparation of NYE | 3,860 | 504 (13.1 %) | 5.72 ms | 15.34 ms | 20.32 ms | 33.9 ms |

Vulkan's low global priority does not protect another program's frames on this driver: the background run cost the
stand-in as many late frames as the old share-time run. Only SteamVR's frame timing paces the steps (the governor
waits for the idle GPU time VRChat's frames leave, and backs off after any late frame), so the background preparation
runs beside VRChat only in VR, and otherwise waits for VRChat to close; a share made meanwhile prepares at once, as
before. With the governor, the impact on VRChat's frames in VR was not measured here (no SteamVR on this PC during the
work): that is the first thing to watch in VR (SteamVR's frame timing, or the service log's
`prepare upload: tree depth …: tuning between VRChat's frames` lines).

## Reproduce

```powershell
$env:CARGO_TARGET_DIR='C:/Users/Enzo/.orbiters-target'; $env:CARGO_INCREMENTAL='0'
cargo build --release -p orbiters-reconstruct --bin orbiters-reconstruct --example eval --example ground --example framegame
# A lab copy of a session: its files hard-linked, without its reconstruction (lab/linkcopy.py), then
orbiters-reconstruct <copy> [--budget N] [--fade-every N] [--unseen-weight W] [--fade-to-the-end]
eval <copy> <copy>/reconstruction/splats.ply --json out.json
ground <session> [<session> …]
framegame <splats.ply> --seconds 20 --json frames.json
```
