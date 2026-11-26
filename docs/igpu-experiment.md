# The window on the integrated GPU (experiment, 2026-10-25)

**Question.** Can the window, its 3D view included, draw on the processor's integrated GPU so that it costs VRChat
nothing on the graphics card?

**Short answer.** Partly. Windows shows a window drawn on the integrated GPU on the graphics card's monitor, but the
graphics card copies every frame to do so, in the window's own process. For the interface that copy costs a GPU-bound
game about twice what drawing the frame on the graphics card costs (scrolling: 3.6 % of its frames instead of 1.6 %;
full screen 8.5 % instead of 3.2 %). The integrated GPU pays off for the 3D view, the window's one heavy load: orbiting
a million splats took 2.6 % instead of 11.4 % (full screen 3.2 % instead of 28 %), but it moves at 21–27 frames a
second instead of 60 and slows memory-bound work on the processor by about 12 %. A window that draws nothing costs
nothing measurable on either GPU, so what the window costs VRChat in practice is how many frames it draws while VRChat
runs. It now draws one frame per change from the service instead of two, and at most five a second of the service's
news while it is behind VRChat (0.5 % from the integrated GPU, nothing measurable from the graphics card, where 30 a
second took 1.7 % and 0.6 %).

## Follow-up: only the 3D view on the integrated GPU (0.3.1)

The window stays on the graphics card and the 3D view's splats draw on the UHD 770 in a thread of their own
(`pages/splat_worker.rs`): each frame is read back and uploaded to a texture on the RTX 4080, which the window shows
(1.6 MB a frame while the camera moves, 0.4 Mpx at most there). Same blocks as below (`presence.py --cases
splats,splats-view-integrated`, 3 repetitions, 2026-10-25), orbiting New Generation (970k splats) in the default
1280×820 window:

| Case | Window frames a second | 3D view frames a second | Change of the load's frames |
|---|---|---|---|
| Window and 3D view on the RTX 4080 | 60 | 60 | −12.1 % (−12.1, −11.3, −12.3) |
| Window and 3D view on the UHD 770 | 27 | 27 | −1.6 % (−1.6, −1.1, −2.1) |
| Window on the RTX 4080, 3D view on the UHD 770 | 60 | 28 | −2.2 % (−2.2, −2.5, −2.0) |

The 3D view on the integrated GPU costs the game about what the whole window there does, and the window around it
stays at 60 frames a second (the bench orbits every frame, so the window draws 60 a second: part of the 2.2 % is
the interface's own frames). Since 0.3.1 Settings, Interface, *3D view on the integrated GPU* chooses *While VRChat
runs* (default: without a game there is nothing to spare the graphics card for, and the RTX 4080 draws 60 frames a
second of it), *Always* or *Never*; the view moves between the GPUs while open when VRChat starts or closes. The
GPU bench takes `--view-gpu always`.

## Machine

Intel Core i7-12700K with UHD Graphics 770 (no monitor on it), NVIDIA RTX 4080 16 GB driving the 2560×1440 monitor
at 59 Hz (100 % scaling), 64 GB, Windows 11 Pro 23H2 (22631), hardware-accelerated GPU scheduling off, NVIDIA driver
32.0.16.1692, Intel 32.0.101.7088. The usual desktop apps stayed open (Chrome, Discord and its clips recorder, Claude,
VRCX, Steam, three Unity editors): about 35 % of the RTX 4080's 3D engine at its idle clocks, DWM included. DXGI also
lists the RTX 4080 a second time, without outputs (luid 0x0bbb559b, Virtual Desktop's display driver).

## How it was measured

- **The window**: `orbiters-desktop --gpu-bench <scenario> --gpu integrated|graphics-card` opens the real window
  (`App`, its pages, `gpu::options`) over the screenshots' stand-in controller (made-up snapshot, the real session
  library, nothing started), never focused, driven by synthetic input. Scenarios: `still` (Settings, no frame),
  `idle` (Home: 2 frames a second, its look at the music), `idle --publish 30` (the snapshot republished 30 times a
  second, as the service does while recording or with N4N0's menu open; `--background` draws it as a window behind
  VRChat does), `scroll` (Settings scrolled, 60 frames a second), `splats` / `splats-still` (the 3D view of a session,
  orbiting or still). Window 1280×820 unless said.
- **A game on the graphics card**: `scripts/gpu-lab` `load` draws a heavy fragment shader over 2560×1440 on the RTX
  4080, frames back to back (8 ms of GPU work each, 113–120 frames a second alone), above normal priority and out of
  Windows' power throttling, as a game in front runs. It stands in for VRChat at its GPU limit.
- **Blocks** (`presence.py`): the load runs for minutes while each case runs for 20 s, with 20 s of nothing between
  cases. A case's change is its frames a second against the mean of the blocks around it, so the background apps'
  drift weighs on both; the tables give the median of 3 repetitions (each repetition in brackets where it matters).
- **Counters** (`gpu-lab sample`): `\GPU Engine(*)\Utilization Percentage` per process, adapter and engine, and
  `\GPU Process Memory(*)`, twice a second for 29 s, without the load. At the graphics card's idle clocks (500 MHz,
  P5) they overstate what a frame costs a busy GPU: the same scrolling window took 14.9 % of the idle RTX 4080 and
  2.5 % of the busy one.
- **The processor**: `gpu-lab cpu alu|mem`, one thread (multiply-adds, or dependent reads over 256 MB), in blocks.
- **The SteamVR overlay**: `gpu-lab overlay` does what `orbiters-openvr`'s `OverlayTextures` does, without SteamVR:
  a D3D11 device on the RTX 4080, two 1024×640 textures written in turn with `UpdateSubresource` of the changed part
  and a `Flush`, 45 times a second.
- Runs happened only while nobody played or used the computer (no VRChat or SteamVR, no video playing, no keyboard or
  mouse for 5 minutes). Earlier runs one after the other, with the load at normal priority, were too noisy (the
  background apps' drift, and stalls of tens of milliseconds in the load) and are not used here.

To run them again: `cargo build --release` in `scripts/gpu-lab` and `cargo build --release -p orbiters-desktop`, then
for example `python scripts/gpu-lab/presence.py out --cases still,idle,scroll,splats` (the load),
`--measure cpu-mem` (the processor), `python scripts/gpu-lab/matrix.py out window sizes` (counters and frame rates).
The 3D view cases open the session whose folder name contains `--session` (here New Generation, 970k splats).

## A GPU-bound game on the RTX 4080

Change of the load's frames a second while the window does this, drawn from either GPU:

| Case | Window frames a second | From the UHD 770 | From the RTX 4080 |
|---|---|---|---|
| Settings open, nothing drawn | 0 | +0.2 % | +0.1 % |
| Home, idle | 2 | +0.3 % | +0.1 % |
| Service news 30 times a second (one frame each) | 30 | −1.7 % (−2.4, −1.5, −1.7) | −0.6 % (−0.7, −0.6, −0.4) |
| Same, the window behind VRChat (at most 5 a second) | 5 | −0.5 % (−0.5, −0.5, +0.2) | 0.0 % |
| Settings scrolling | 60 | −3.6 % (−3.6, −3.0, −3.8) | −1.6 % (−1.6, −1.3, −3.4) |
| 3D view open, still | 0 | +0.4 % | −0.1 % |
| 3D view orbiting 970k splats (New Generation) | 27 / 60 | −2.6 % (−3.0, −2.6, −2.0) | −11.4 % (−11.9, −10.9, −11.4) |
| Full screen (2560×1440), scrolling | 60 | −8.5 % (−9.9, −8.4, −7.9) | −3.2 % (−3.9, −3.2, −3.2) |
| Full screen, 3D view orbiting 970k splats | 21 / 60 | −3.2 % (−3.6, −3.2, −2.8) | −28.1 % (−28.1, −28.7, −27.7) |
| Scrolling, DirectComposition swapchain (`WGPU_DX12_PRESENTATION_SYSTEM=Visual`) | 60 | −5.1 % | −1.2 % |
| 3D view orbiting, DirectComposition | 27 / 60 | −2.5 % | −11.8 % |

The copy grows with the window's pixels; a DirectComposition swapchain does not make it cheaper.

The SteamVR overlay's uploads (on the RTX 4080, where SteamVR's compositor is) cost −1.5 % for the whole 1024×640
picture 45 times a second (−1.5, −2.0, −1.5) and −0.6 % for a 240×60 part (a button's highlight).

## What the counters say (no load)

| Case | Frames a second | From the UHD 770: its process on the UHD 770 / on the RTX 4080 | From the RTX 4080 |
|---|---|---|---|
| Home, idle | 2 | 0.5 % / 0.5 % | 0.3 % |
| News 30 a second (two frames each, before) | 59 | 11.7 % / 12.8 % | 9.0 % |
| Focused (decorations at 15 fps) | 16 | 3.7 % / 2.2 % | 1.6 % |
| Settings scrolling | 60 | 18.0 % / 10.6 % | 14.9 % |
| 3D view orbiting 970k splats | 19 / 60 | 98.7 % / 3.3 % | 40.4 % |
| 3D view orbiting 223k splats (Canine) | 30 / 60 | 99.3 % / 6.3 % | 46.1 % |

The share of the RTX 4080 that a window drawn on the UHD 770 takes is the copy, in the window's process, on the 3D
engine. GPU memory of the window's process: from the UHD 770, 36.6 MB dedicated and 21 MB shared on the RTX 4080 for
the presentation (70 + 53 MB full screen) and 50 MB (interface) to 468 MB (970k splats) of system memory; from the
RTX 4080, 42 MB (interface), 351 MB (970k splats), 596 MB (970k splats full screen). The 3D view loaded in 0.3–0.9 s on
both.

## The 3D view on the UHD 770

Frames a second while orbiting, before the change below (the RTX 4080 draws 60 in every case):

| 3D view's pixels | 970k splats | 223k splats |
|---|---|---|
| 640×370 (0.24 Mpx) | 34 | 59 |
| 960×575 (0.55 Mpx) | 26 | 42 |
| 1280×780 (1.0 Mpx, the window's size) | 19 | 30 |
| 1920×1190 (2.3 Mpx) | 12 | 18 |
| 2560×1400 (3.6 Mpx, maximized) | 8 | – |

Much of the time goes to the splats themselves (projection and sort), not the pixels. The 3D view now draws at most
0.4 Mpx while the camera moves on an integrated GPU and is drawn whole once it has stood still for a quarter of a
second: 970k splats at 27 frames a second in the default window, 21 full screen (where the rest of the window costs
more too).

## The processor

The UHD 770 shares the processor's memory. One thread's dependent reads over 256 MB (memory-latency-bound work, as
much of a game's main thread is), in blocks like the load's:

| Case | From the UHD 770 | From the RTX 4080 |
|---|---|---|
| 3D view orbiting 970k splats | −12.3 % (−12.3, −15.5, −11.9) | −3.6 % (−5.8, −0.8, −3.6) |
| Settings scrolling | −2.5 % (−2.2, −6.4, −2.5) | −4.5 % (−4.5, +0.4, −4.9) |

The window's own processor work at 60 frames a second costs such a thread a few percent from either GPU; the 3D view
drawn by the UHD 770 adds about ten points, its splats read and written in the same memory. Arithmetic alone (one
thread's multiply-adds) does not notice: −0.4 % and +0.05 % with the 3D view orbiting.

## A real session (watched, not touched)

Enzo's own VRChat session in VR on this machine (Virtual Desktop to a Quest, SteamVR), 2026-10-25 from 02:37 to 03:13
CEST, counters every 2 s, about 25 minutes of it with VRChat running: VRChat used 35 % of the RTX 4080's 3D engine on
average (median 34 %, p95 61 %; the GPU was not the limit). His window was open, opened before VRChat, so on the RTX
4080 under the old rule, and with two frames per change: 0.40 % of an engine on average (median 0.14 %, p95 1.7 %, at
most 5.3 %). The SteamVR overlay's process never drew (its tab was not shown).

## What changed

- **Settings, Interface, Window graphics**: *Integrated GPU* or *Graphics card* (default since 0.3.0), for the window and its 3D
  view. The window chooses when it opens (eframe cannot move a window to another GPU), so a change applies the next
  time it opens, and the row says which GPU draws it now. Computers without both kinds of GPU do not show the row and
  draw on the adapter of the primary display, as before. DX12 reports every adapter as able to present to the window,
  so there is nothing to check beforehand; Windows' hybrid presentation did it on this machine for every test.
- **The 3D view** on an integrated GPU draws at most 0.4 Mpx while the camera moves (above).
- **Fewer frames**: egui's `request_repaint` draws two frames, to let layouts settle; the service's news now asks for
  one (`anim::wake`: 30 frames a second for 30 news instead of 59). While VRChat runs and the window does not
  have the focus, the service's news is drawn at most five times a second (`ipc::BACKGROUND_PACE`).
- **The setup** asks wgpu for the least power-hungry GPU (the integrated one where there is one).
- Tools: `orbiters-desktop --gpu-bench`, `scripts/gpu-lab` (`gpu-lab.exe`, `presence.py`, `matrix.py`, `run.py`,
  `analyze.py`, `table.py`).

## Not measured

- VRChat itself under these loads: the load stands in for a GPU-bound VRChat. In VR a missed frame is a reprojected
  one, so near the limit a 2 % smaller frame rate can mean more than 2 % of late frames.
- SteamVR's own side of the overlay: its compositor drawing the overlay in the headset, and `SetOverlayRaw` (which
  makes SteamVR create a texture and upload the whole picture on every call, so at least the −1.5 % above). SteamVR was
  in use earlier and, without a headset, would not have drawn the overlay anyway.
- Laptops, where the integrated GPU drives the panel: there the window on it needs no copy, and the graphics card's
  frames are the ones copied.
- A 4K monitor: the copy grows with the window's pixels (a maximized 4K window draws 2.3 times the pixels of 1440p).

## Recommendation

Decision (Enzo, 2026-10-25): the graphics card is the default, since drawing the interface there costs a game less than
the copy from the integrated GPU; the integrated GPU stays a choice, and is worth it for the 3D view alone.

The experiment's own recommendation was to keep the integrated GPU as the default, with the frame reductions. In the cases that come up while playing (the window
closed, idle, or behind VRChat while recording or with N4N0's menu open) the window now costs a GPU-bound VRChat at
most about half a percent of its frames from the integrated GPU and nothing measurable from the graphics card, and the
3D view, the one load that can take a quarter of the graphics card, stays off it. The price: interacting with the
window costs the graphics card about twice what drawing it there costs (more the bigger the window), the 3D view
moves at 21–27 frames a second instead of 60 (smaller while it moves, sharp when it stops), and while it moves it slows
memory-bound processor work. *Graphics card* in Settings suits someone who uses the 3D view when VRChat is closed.

Next, if wanted: the same pacing whenever the window does not have the focus (not only while VRChat runs); the 3D view
at about 15 frames a second while VRChat runs, on either GPU; the overlay at 30 uploads a second instead of 45 (its
whole picture costs 1.5 % at 45, a button's part 0.6 %; `SetOverlayRaw` would upload the whole picture every time).
