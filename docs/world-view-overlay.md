# 3D worlds in the SteamVR overlay (2026-10-29)

**What it does.** N4N0's menu in the play space (the SteamVR overlay) has a pill under it, "3D worlds". It opens a
panel in the menu's place: a search field (the panel's own keyboard, or the computer's keyboard as the menu's search
takes it) over the worlds members captured and shared, as cards with each world's thumbnail. Pressing a card downloads
that capture and shows it around you: its ground on your play space's floor, its start view where you stand, facing the
way you face, appearing from there outward. Its page then has Hide, Recenter, See VRChat (the capture at 55 %, VRChat
behind it) and how sharp it is (Light, Balanced, Sharp). The capture stays around you after the menu closes; the pill
then says which one shows, a press away from its page. Code: `crates/orbiters-desktop/src/overlay/worlds/`.

**Same captures, same placement as the VRChat world.** The panel reads what "N4N0 - 3D picture viewer"
(`Assets/N4N0Viewer/Scripts/SplatMenu.cs`) reads: `GET /maps/vrchat/previews`, searched as its menu searches (the
world's name, its author or who shared it containing what is typed). It loads the same file the world's PC renderer
loads, the slot's PC tree (`files.orbiters.cc/public/vrchat/slots/<slot>/pc`, key checked against the listing as the
world does) and the slot's `sky.jpg`, and draws the tree's leaves (its finest splats: 786 432 for VRChat Home). On a
development server, whose slots are published to storage the app has no address for, it reads the preview's web
`.splat` and sky map through the API instead. Placement is `SplatViewer.AlignToView` with the anchor at the head's
place on the floor (Y 0 of SteamVR's standing universe) and the head's level facing: the start view's eye over the
head, its `floor` on the floor. One difference: a preview without `floor` (all five public ones today) keeps its own
heights in the world, the world's ground being its anchor; here there is no world ground, so its start eye stands
1.6 m (its `eyeHeight`, else the contracts' default) over the floor, where a standing person's eyes are.

## How a 3D capture shows in SteamVR while VRChat owns the scene

SteamVR gives the scene to VRChat; another process can only show overlays: textured quads the compositor draws over
the scene. Options looked at:

| Option | Head turns | Head moves | Cost | Verdict |
|---|---|---|---|---|
| A head-locked side-by-side overlay drawn every frame from the predicted pose | swims with every prediction error, and a late picture moves the whole world with the head | right | a full stereo view at 90 Hz | no: any late picture is felt |
| `SetOverlayTransformProjection` (a projection overlay per eye) | as SteamVR reprojects it (undocumented) | as drawn | per frame | no: undocumented, nothing to verify it against without a headset |
| `Panorama` / `StereoPanorama` flags | – | – | – | legacy flags, no documented behaviour |
| **World-locked side-by-side quads forming a cube around the head** | **exact at the headset's rate** (see below) | parallax follows the batches | only when the eyes move | **chosen** |

**Why a world-locked cube is exact for head turns.** Each face of the cube is a quad fixed in the play space, its
texture side by side (`VROverlayFlags_SideBySide_Parallel`: the left half to the left eye, the right half to the right
one). Each half holds the capture projected from that eye's position onto the face's plane (an off-axis pinhole camera
through the face, `stereo.rs` `Cube::camera`). Such a picture is right for every direction the eye looks from that
position: SteamVR's compositor draws the quads with the latest head pose every frame, so turning the head is exact at
90 Hz whatever rate the pictures are made at, and a late picture never moves the world with the head. Only moving the
eyes needs new pictures (parallax), and a picture made from a slightly old place is only slightly wrong.

**The cube.** Six faces 1.5 m from its centre (the eyes may stand anywhere inside: the projections stay exact), each
1024 + 2 × 4 texels a side per eye (Balanced; Light 768, Sharp 1440), overlapping their neighbours by 8 texels so
SteamVR's filtering never shows a seam. When the eyes wander 45 cm off its centre, a second cube around the head draws
its six faces and takes over (shown, then the old one hidden: SteamVR shows them in either order without a gap). The
cube sits under N4N0's menu and the pill (sort order 0 against 10).

**A batch.** Drawing every face an eye sees for both eyes would be ten views of the splats a batch (an Index's eye sees
4–5 faces: its field of view, about 108°, is wider than a face's 90°). Instead each batch draws **one wide view per
eye**: the eye's own field of view (`GetProjectionRaw`, `GetEyeToHeadTransform`, the pose predicted to when the batch
lights the displays: `GetTimeSinceLastVsync`, `Prop_SecondsFromVsyncToPhotons_Float` and the batch's own time) widened
6° each side, then **resamples** it into the faces it covers (`resample.wgsl`: a homography per face and eye, exact as
both are projections from the same eye; texels it does not cover keep what they had). A new cube's faces, and now and
then a face out of view, are drawn whole instead. A batch goes once an eye moved 2.5 mm or the head turned 4°, at
most 20 / 30 / 45 a second (Light / Balanced / Sharp): a still head costs nothing.

**Handing pictures to SteamVR without leaving the GPU.** The splats draw on a wgpu device (D3D12) on the headset's
adapter (`GetDXGIOutputInfo` → LUID). Each face texture is a shared D3D12 resource (simultaneous access, so it is back
in the common state after each submission) opened by a small D3D11 device on the same adapter, which hands it to
SteamVR with `SetOverlayTexture` (the D3D11 path every overlay of the app uses). Each face has two textures used in
turn; before writing one, the GPU waits on a shared fence the D3D11 side signals after handing textures over
(`interop.rs`).

**VRChat first.** Before each batch the overlay reads SteamVR's last frame timings: after a frame dropped or
reprojected for the GPU, no batch for 50 ms, then twice as long each time up to 2 s (the training governor's rule,
`orbiters_reconstruct::governor::verdict`); parallax then lags, VRChat keeps its frames.

**The reveal.** The capture appears from its start view outward over 0.9 s, as the world's reveal does: its splats are
ordered by distance from the start eye and each batch draws those within a radius growing on a log scale; the
overlays fade in over 0.45 s meanwhile, and out over 0.35 s when hidden.

## Measured (offline, RTX 4080, VRChat Home's PC tree, 786 432 splats)

`orbiters-desktop --world-view <out> <pc tree> [--load <s> [--still] [--quality light|balanced|sharp]]`. The computer
was in use meanwhile (two Unity editors, browsers, the installed app: 15–20 % GPU at rest), so absolute times vary by
run; comparisons are within a run.

**Correctness** (an Index-like headset, 1440 × 1600 an eye; each eye's picture composed from the faces as SteamVR
would, against the renderer drawing that eye's view directly; PSNR over RGB):

| Case | PSNR |
|---|---|
| Standing at the start view, faces drawn whole | 33.4 dB (left), 33.7 dB (right) |
| Head turned 35° and tilted 20° down, faces not redrawn | 40.6 dB |
| Looking down 70°, faces not redrawn | 49.8 dB |
| A wide batch after turning 25° and moving 1.4 cm | 40.9 dB (left), 41.2 dB (right) |
| Eye moved 2 / 5 / 20 / 50 mm, faces not redrawn | 32.7 / 30.7 / 26.3 / 23.3 dB |

Faces drawn whole differ from the direct render mostly by the splats' screen-space approximation (each face camera
looks another way than the eye); a wide batch is drawn looking the eye's way and comes closer. The faces show no seam
(`eye-left.png`, `stereo.png`). The shared texture read on the D3D11 side was byte for byte what D3D12 drew.

**Cost of a batch** (median of 12, the head turning 3° between batches; the wide view's size per eye):

| Quality | Faces (per eye) | Wide view | Batches a second at most | A batch on the GPU |
|---|---|---|---|---|
| Light | 776 × 776 | 1.15 Mpx | 20 | 5.7 ms |
| Balanced | 1032 × 1032 | 2.05 Mpx | 30 | 6.4 ms |
| Sharp | 1448 × 1448 | 4.05 Mpx | 45 | 9.0 ms |

Drawing the faces an eye sees directly instead (4–5 faces, both eyes) took 8–13 ms **a face**, 40–65 ms a batch: most
of a view's time is the splats' projection and the sort of their tile entries, whatever its pixels (a face at 776 or
1448 pixels across cost about the same), so fewer views matter more than fewer pixels. A new cube (six faces drawn
whole, both eyes) takes about 50–80 ms once.

**Cost to a game on the same GPU.** As docs/igpu-experiment.md measured the window: `gpu-lab load 4080 20 --work 1600`
(a GPU-bound stand-in, about 8.6 ms a frame at 2560 × 1440, back to back) alone, then beside `--world-view --load 26`
(the overlay's batches with the head turning ±35° and swaying ±4 cm every few seconds: never still, so a batch is
always due), two rounds, frames a second of the stand-in:

| Beside it | Round 1 | Round 2 | Change |
|---|---|---|---|
| nothing (three blocks around them) | 100.9 | 99.4 | – (98.2–104.0 over all blocks) |
| a still head | 102.8 | | none measurable |
| Light, moving | 93.3 | 92.7 | −6.6 % |
| Balanced, moving | 90.9 | 89.4 | −9.4 % |
| Sharp, moving | 86.5 | 83.4 | −14 % |

Beside the stand-in the batches ran 16–18 a second (16–24 ms each, sharing the GPU). The first version, drawing each
seen face directly at up to 45 batches a second, cost the same stand-in 20–30 % of its frames. A stand-in at 90 Hz with
headroom (`framegame`, 4.5 ms frames) lost no frames beside it (late frames 0.1–0.3 % with or without; its 95th
percentile frame 1–2 ms longer). For comparison, the window's 3D view orbiting a million splats costs the GPU-bound
stand-in 11.4 % (igpu-experiment.md). In VRChat the governor's back-off adds to this: batches stop while VRChat's
frames come late.

**Memory.** 515 MB of graphics memory held by the process with a capture shown at Balanced (DXGI's local video memory
of the offline check: the splats and the renderer's buffers, its sort included, twelve face textures, six face
pictures and two wide views), all freed when it is hidden (the render thread and its device end).

## What it covers, and walking

Overlays draw over the whole scene: while a capture shows, VRChat's world, the other players, your own avatar and
hands are behind it (See VRChat shows them through at 45 %). The capture is fixed in the play space: you walk through it
by walking in your room; VRChat's own locomotion (the stick) moves VRChat's world behind it, not the capture.
Recenter puts its start view where you stand again.

## Next, if it is kept

- A quad splat renderer like the world viewer's (each splat a quad, sorted once per head move by distance on the
  processor, both eyes and every face from one order): no tile-entry sort per view, which is most of a view's cost now.
- The tree's level of detail (its merged boxes for far places, cut from where the head stands, as the world draws it):
  fewer splats per view.
- Hands through the capture: holes where the controllers are, so VRChat's hands show.
- The avatar's projected screen (players who use N4N0's screen never see the overlay's menu, so never the pill): a new
  screen version with a 3D worlds page.

## What was not tried in a headset

Nothing of this ran in SteamVR: no headset was reachable, and the person's own SteamVR and app were left alone. Unknown
until tried: whether SteamVR takes a side-by-side overlay's height from half the texture (assumed: square faces;
otherwise the faces would be half as high and the cube would gape), the shared texture's hand-over (SteamVR's
`SetOverlayTexture` with a D3D11 texture opened from a D3D12 resource), the panel's laser input and the pill, the
computer's keyboard reaching the panel, the predicted poses' timing, how the cube looks with VRChat behind it at 55 %,
and what the batches cost VRChat's own frames (the governor's back-off included).
