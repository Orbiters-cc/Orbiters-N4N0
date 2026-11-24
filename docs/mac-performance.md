# VRChat on a Mac: measuring its frame rate

How to measure VRChat under Obelisk's Linux engine on a Mac so that two conditions can be told apart, and what has been
measured so far. The script is `tools/mac-bench/bench.py`; its results files are in `docs/mac-bench-results/`.

## The three frame counters (R3)

Each lives in the engine's log (`~/Library/Logs/Obelisk/Steam/wine-438100.log`) and the script keeps them apart:

| Counter | Line | Says |
|---|---|---|
| Guest Present (production) | `neptune unixlib wsi (wsd): <n> presents (<m> shown events), last 60: interval avg X ms, fence wait avg Y ms` | X: the time between the game's submissions, averaged over 60. Y: how long each submission waited for the host (Neptune's decoder, DXMT's encoding, the GPU) to finish it |
| Worker shown events | the `(<m> shown events)` of that line | callbacks from the window server, zero-time ones included |
| Window-server presentation | `metal-HUD:` lines (raw per-frame intervals) and `wsd: window 1 overlay: 120 frames shown, ...` | what reached the screen |

A new swapchain generation starts at each `guest swapchain: created` line; the script lists them with their time. It keeps the
raw HUD intervals (mergeable across sessions) rather than per-second minima. **The window server presents nothing while the
screen is locked, the window covered or on another Space**: then the shown counters are 0 and only production can be measured.

Not done yet: the CLI does not expose these counters per generation in the Game session record (`played.frames` is the HUD's
shown frames only). Reading them afterwards means reading the engine's log, or this script's results file.

## The procedure (R4)

The script records, per start: `pmset -g custom` and `-g batt` (Low Power Mode on AC and on battery, the power source),
`pmset -g therm` at start and end, the render percent the window server applied, the swapchain sizes and generations, and
once a second whether the session lives (its record and its launcher), the console's lock state (`ioreg` `IOConsoleLocked`),
the frontmost app (`lsappinfo`), GPU utilisation (`ioreg`, device-wide) and the CPU of the threads of its own launcher's
process tree (`ps -M`). It starts `obelisk-cli steam play 438100 [--render-percent N]
-- -screen-fullscreen 1 -window-mode borderless <link>`, waits (with deadlines) for the engine to say the game runs and for
the local player to be in **the destination of the link** (world, instance and every tag it names: a fallback is no
arrival) as **this start's own** VRChat log says it (made since the start, one of the arguments its head lists being the link; held open
from then on), waits `--warm`
seconds and then until 20 s pass without a frame held over 250 ms (within 180 s, else nothing is scored), records `--windows` windows of `--window` seconds
with nobody touching the game, takes 3 s stack samples only after them, and in every case (an arrival that never came, a
failed start, an interruption) quits by session (`steam quit --session --wait 60`, never forced) and writes its results
file with its `outcome`. The session is the one **the program started here names in its own `starting` event** (obelisk-cli 0.1.10),
the only one ever asked to quit, whether or not that program still lives; its own record says how it ended (`terminal`). A
`running` event that names another session (the program found somebody else's again) is an error: nothing is asked to quit, and
the series stops. A run
whose session is not known to have ended, or that may have started one it was never told the name of, exits with 5, and `session.sh` stops the series there, as it does after any
run that failed.

A window is **valid** only when all of it was seen: from its start to its end, never more than 5 s between two looks, the
session lived, the screen was unlocked, this session's own window server was the frontmost app and the local player stayed
in the destination; its submissions were read over at least 90 % of it, inside it, never more than 5 s apart (the line before
the window is the rate's base and no coverage), and the shown frames of that window server (by
the process and the time its `metal-HUD:` lines carry) fill as much of it, never more than 5 s apart; otherwise it is kept
with `valid: false` and why, and never scored. `--allow-locked` scores production alone behind a locked screen, and says `production_only` in each window. Rates
are counted, never reciprocal intervals: the presents (or shown events) between two lines of one swapchain generation
divided by the time between those two lines (lines read together are one reading, the last of them: two batches read at
once used to count one), the last line before the window included as its base, a count that went
back (a new swapchain) never subtracted across, a measured zero kept as 0.

Everything that decides a window is kept with its times in the results file (`observed`: the looks, the moves, the log's lines;
each window's `from` and `to`), so that it can be scored again.

`tools/mac-bench/test_bench.py` checks the scoring (boundary interval, generations, zero, coverage, validity), the arrival
and the cleanup, without a game. **Results made by an earlier `bench.py`** (every file up to 2026-11-23) were scored by
weaker rules: a window valid there is not one valid here.
`tools/mac-bench/summary.py` gives per condition the median over starts (the start is the unit) and the spread from the
lowest to the highest start; a condition with one start says "one sample".

### The next session (the verifier's list; one command)

With the Mac unlocked, plugged in, idle and cool for 5 minutes, nothing else in front, the owner runs:

```sh
tools/mac-bench/session.sh 'vrchat://launch?ref=orbiters.cc&id=wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:<5 digits>~private(<his usr_ id>)~region(eu)'
```

and leaves the Mac alone for about 50 minutes (eight starts of about six minutes, 90 s apart). It plays the engine
percents `58 100 87 67 45 87 100 58` (`PLAN="..."` to change them; eight starts at most): every condition, 58, 100 and
87 twice, alternated, including engine 87 % (Home's panel-capped "100 %", never measured so far) and one below 58 (45).
It refuses to start while the screen is locked. To be held constant by hand: the world and a private instance of the
owner's, nobody else, his avatar as it is, the camera where the player spawns, VRChat's graphics settings, display mode and
refresh, power source, Low Power Mode (say which: `pmset` records it), Game Mode (on, as the full-screen game gets it;
`launch settings` of the installation untouched), the warm-up. Then: `python3 -I tools/mac-bench/summary.py
docs/mac-bench-results/<date>*.json`.

What this session still cannot measure (the script says so in `not_measured`): GPU frequency and command-buffer start and
end times (powermetrics and Metal's counters need root or the game's own process), queue depth, host threads' runnable and
wake-up intervals. Without them the host wait cannot be split into GPU execution, queueing and wake-up; an overlap test
(more work in flight) needs a change that demonstrably changes the in-flight work, and one matched pair of the same
condition with the screen unlocked and locked is still to make.

## The host timeline, and what owned workloads say (2026-11-23, 12:56 to 14:45; no VRChat start)

What splits the fence wait, measured rather than guessed: the branches `perf-timeline` of Orbiters-cc/voile-lkd (from
`3a299f2`, the shipped `orbiters-cli-0.1.8` line), Orbiters-cc/neptune, Orbiters-cc/dxmt (a public fork of Zagrios/dxmt; its
branch `orbiters` is `b7d97ee`, what Voile ships) and Orbiters-cc/Obelisk (from 0.1.9). **Nothing of it is released**; no
engine file, manifest or CLI of a player changed.

- **On**: `OBELISK_TIMELINE=<an existing folder>` for obelisk-cli of that branch, with the instrumented engine files; each start
  gets a folder in it, `<date>-<time>-<pid>/otl-{npt,dxmt,vtest,wsd}-<pid>.bin`. `OBELISK_TIMELINE_PASSES=1` adds GPU times per
  encoder. `launch.sh` takes the variable away before the game: the guest never has it.
- **Read**: `python3 tools/timeline/otl.py <that folder> --passes --cadence` (voile-lkd; `tools/timeline/README.md`).
- **Off**: one `getenv` per host process and a branch per site; no thread, file, descriptor or memory
  (`tests/timeline/run-otl-test.sh`).
- **Joined end to end: 157,517 of 157,517 frames** of the owned workloads, through the real guest driver, ring, DXMT and `wsd`.
- **Its price** (ms a frame, 2940 x 1912, quiet runs of one five-minute stretch; `tests/graphics/results/2026-11-23/overhead.txt`):

| Frame | shipped files | instrumented, off | on | on, with GPU times per encoder |
|---|---|---|---|---|
| trivial | 0.509 | 0.505 | 0.508, 0.531 | 0.503, 0.507 |
| 2x scene and resolve | 0.831 | 0.830, 0.832 | 0.832, 0.847 | 0.834, 0.841 |
| 2,000 draws, 18,000 commands | 1.680 | 1.692, 1.709 | 1.716, 1.757 | 1.717, 1.734 |
| GPU-bound | 9.387 | 9.393, 9.685 | 9.391, 9.555 | 9.387, 9.448 |

  Off: within run-to-run noise (at most 0.03 ms on the heaviest decode). On: 0.02 to 0.06 ms on 2,000 draws. The runs beside
  another program's compile are in the file and count for nothing.

### Experiment 1, display cadence: Low Power Mode **off** only

`wsd` (the timeline's build) as a full-screen app of the games category, numbered moving frames from the owned client, AC, screen
unlocked, window in front. Two sessions; the second:

| Sent per second, size | Shown per second | Replaced | Display-link ticks per second | `nextDrawable` p50 / p99 | Shown intervals, in refreshes |
|---|---|---|---|---|---|
| 20, 2940 x 1912 | 20.0 | 0 | 60.0 | 0.07 / 0.53 ms | 3: 294, 2: 2, 1: 1, 5: 2 |
| 32, 2940 x 1912 | 32.0 | 0 | 60.0 | 0.01 / 0.08 | 2: 415, 1: 62, 3: 2 |
| 60, 2940 x 1912 | 60.0 | 0 | 60.0 | 0.01 / 0.01 | 1: 898 |
| 20, 1705 x 1109 | 20.0 | 0 | 60.0 | 0.01 / 0.03 | 3: 288 |
| 32, 1705 x 1109 | 32.0 | 0 | 60.1 | 0.01 / 0.06 | 2: 418, 1: 60 |
| 60, 1705 x 1109 | 59.0 | 15 (phase) | 60.0 | 0.04 / 0.07 | 1: 868, 2: 15 |

**With it off, `wsd` loses nothing**: 60 ticks a second, no drawable wait, every ready frame shown at the next tick. 32 a second
shown as two refreshes (and one now and then) is what 32 in 60 is; the "three refreshes" of VRChat at 100 % are late arrivals,
not `wsd`. All five of macOS's display links give 60 (`tests/graphics/display_link_probe.m`). **The 30 a second under Low Power
Mode is not answered**: it was off, and stays the owner's to turn on. `wsd` is therefore unchanged. To be run, once, unlocked,
hands off for three minutes:

```sh
sudo pmset -a lowpowermode 1; ~/.orbiters-forks/src/voile-lkd-perf/tests/graphics/run-low-power-session.sh ~/Library/Logs/Obelisk/low-power-on; sudo pmset -a lowpowermode 0
```

30 ticks a second with ready frames on the default link and 60 on one that asks for the rate: `wsd` can fix it. 30 on all five:
macOS's policy. 60 ticks and slow drawables: the drawable path.
Also seen: 32.8 ms (two refreshes) from tick to shown at any rate, 49 ms when three drawables queue: latency, not frame rate.

### Experiment 2, pass costs (Low Power Mode off)

`pass_costs.exe` through the engine's own Wine in a temporary prefix and runtime. Milliseconds a frame (time between frames, GPU
at 95 to 100 %); 2940 x 1912 cool (13:16 to 13:22), 1705 x 1109 the best of three rounds, with the encoders Metal got:

| Frame | 2940 x 1912 | 1705 x 1109 | Encoders, nominal tile and copy traffic at 2940 x 1912 |
|---|---|---|---|
| trivial (clear, one triangle, present) | 0.507 | 0.250 | 2, 68 MB: `RGBA8 clear/store` 0.11 ms, the export copy (blit) 0.30 ms |
| and four full-size copies | 1.172 | 0.431 | 0.166 and 0.045 ms a copy |
| scene 1x with depth, then to the back buffer | 0.629 | 0.292 | 3, 140 MB |
| 2x scene, resolve | 0.836 | 0.318 | 3, 214 MB: resolve folded: 2x colour `clear/store+resolve`, depth and stencil `clear/store` |
| the same and `DiscardView` after, or before | 0.837, 0.839 | 0.316, 0.320 | the same three: **the hint does nothing** |
| a `Flush` between scene and resolve | 1.102 | 0.447 | 4: the resolve its own pass, `load/store+resolve` |
| a grab in the middle of the 2x scene | 1.334 | 0.612 | 4, 438 MB: the second part `load/store+resolve`, depth `load/store` |
| eight small targets between parts, nothing read / read | 0.743 / 0.743 | 0.647 / 0.644 | 11: the parts **merged** into one full-size pass |
| eight full-size parts, merged / split by reads (1x) | 1.474 / 3.087 | 0.732 / 1.359 | 11 / 19, 145 / 954 MB: **0.20 / 0.08 ms a split** |
| the same, 2x, split by a resolve each | 1.871 / 4.971 | 1.119 / 2.407 | 11 / 19, 218 / 2,017 MB: **0.39 / 0.16 ms a split** |
| eight uploads into a texture the pass reads | 0.928 | 0.816 | 18: each upload splits the pass (order requires it) |

The same passes in plain Metal (`pass_bench.m`, per pass, flat / noise content): storing the 2x colour beside the resolve costs
0.17 ms (cool) / 0.5 ms (noise, warm) at 2940 x 1912, 0.02 to 0.17 at 1705 x 1109; a split 0.10 / 0.4 ms at 1x, 0.22 / 0.9 ms at 2x;
storing depth of one value nothing. Noise bounds it above, a picture lies between.

What it decides:

- **The chain's own fixed cost is small**: a trivial frame's fence wait is 0.50 ms at 2940 x 1912 (GPU 0.40, GPU end to
  notification 0.08, notification to the guest's wake 0.02). It cannot be the 45 ms; a busy game's queues are not in it.
- **The export copy is about 0.17 ms** (0.30 alone) and `wsd`'s full-screen triangle 0.5 to 1.0 ms of GPU a shown frame.
- **DXMT's optimiser does what it should here**: matching resolves fold, independent parts merge. Left: real data
  dependencies, and stores nobody reads again. **Discard hints would give VRChat nothing**: DXMT logs "err: Not implemented" at the
  first call, the owned client made it do so, and neither kept VRChat log has that line.
- **Needless stores and splits are several times cheaper than their nominal traffic**: five avoidable milliseconds of VRChat's
  36 would need some thirteen to twenty-five needless full-size 2x splits a frame, six if nothing of them compresses. No longer the presumed main cost; VRChat's own
  encoder list, which the timeline now prints, says.
- **An attachment stored though the next frame clears it** (the exact case: `checkEncoderRelation` always makes a folded resolve
  `StoreAndMultisampleResolve`): DXMT cannot know inside the command buffer that the next one clears. Within one command buffer a
  later `Clear` of the same view could turn it into `MultisampleResolve`; across two it needs the next chunk in hand or a
  guess that corrupts when wrong. Not changed; worth 0.2 to 0.5 ms a 2x scene.

### The GPU's speed is not a constant of this Mac

The same GPU-bound work, offscreen, nothing else running (`pass_bench --sustain`, thermal state "fair" throughout, which
`pmset -g therm` does not say): 8.2 ms a frame at first, **11 to 12 ms after 75 to 120 s**. And: 10.3 to 10.6 ms warm, **13 to 14.8 ms
within 10 s of three busy CPU threads, 10.6 to 10.9 ms within 15 s of their end**. The GPU-bound Direct3D frame took 9.4 ms at
13:31 and 15 to 17 ms an hour later on a warm Mac with nothing else busy. One power and heat budget on a fanless Mac: **CPU
work costs frame rate even where the GPU is the limit**. N4N0 itself used 22 to 28 % of a core all along (`orbiters-desktop`, its
window open). Every row above and every earlier VRChat row is at whatever clock the Mac then had; a comparison of two builds or
settings has to be interleaved and at one thermal state, and the bench should record `NSProcessInfo.thermalState` and the
CPU of everything else beside each window (it records neither yet).

### What N4N0's own window cost (2026-11-23, fixed)

The 22 to 28 % of a core was the window drawing at the display's rate for as long as it was open: on a Mac it asked, in every
pass, for a paint of the hidden window, and egui answers anything asked of a window with a frame. Covered, where no display
holds the rate back, it was far worse. It now draws only when something asks and it can be seen (`mac_cover.rs`: while
eframe holds it hidden it looks at the windows macOS lists, every second, every three beside a game). Measured with a release
build over made-up state in a temporary folder (`orbiters-desktop --link-smoke --no-panels --hold 48`, CPU time of the
window's process over 10 s, nobody touching it, the Mac busy with builds):

| Window | before | after |
|---|---|---|
| shown, behind another app, nothing changing | 22 % of a core | 4.8 % |
| the same, the service saying something new five times a second | 23 % | 6.9 % |
| covered, nothing changing | 137 % | 0.0 % |
| covered, five times a second | 137 % | 1.3 % |

Since the ninth verification (2026-11-23, not measured again) "covered" is only what is certain: the window is not on screen, or
a full-screen game's is over it. Where that cannot be told (no list of windows, another app's window over it, which may be clear in
places) it is drawn once a second; and once, at once, when macOS says something that may have it show (in front, back from the
Dock, the Space, the displays) or what it shows of the service's state changes, at most four times a second.

The service's own looks (3 to 5 % of a core with nothing open, seen by thread on the running app, not measured again) are fewer:
the engine's session records once a second instead of ten times, every process on the Mac every 30 s instead of 3, and beside
a game the network every 2 s instead of twice a second and the processes' CPU every 15 s instead of 3.

### What cuts a frame, a sharper upscale, presentation, `spindump` (2026-11-23, 23:05 to 23:45; no VRChat start)

After three VRChat starts with the timeline (22:20 to 22:41: 39.2 shown frames a second at 45 % per side, 31.3 at 58, 18.6 at 100, the
GPU the limit; the main scene drawn in 4 to 8 encoder pieces too many, some 90 % of the cuts on command-buffer boundaries). Branches
`perf-timeline` of Orbiters-cc/voile-lkd, neptune, dxmt and Obelisk; nothing released. All numbers, conditions and commands:
voile-lkd's `tests/timeline/results/2026-11-23-switches.md` and `docs/wsd-switches.md`.

- **DXMT cuts the scene for a query nobody waits for.** The timeline now records what committed each command buffer and ended each pass
  (`otl.py --splits`). The owned client (266 draws on a blended 2x target, 21.5 ms of GPU) runs at 46.4 frames a second; **with one event
  query ended a frame, 36.4**: 2.1 cuts of the target a frame, all by DXMT's stall flush. Neptune's host looks at every ended query every
  100 us without asking for a flush; DXMT, after 64 looks (6.4 ms), flushes at each look until the GPU reaches the query. VRChat's own
  recordings have its print: 40 % of the gaps between command buffers are 100 to 120 us at 45 %, hardly any is shorter; 17.5 command
  buffers a frame beside two `Flush`. On a hot, busy Mac (thermal pressure 2, as in those starts) the same pair gave 24.5 and 14.2
  frames a second, with 5.8 cuts (disturbed runs: the direction, not a size). A cut costs this client 1.6 to 3.6 ms (blended content; flat scenes: 0.4). An occlusion query ended
  in the middle of a scene with target changes cuts it too; timestamps and readbacks do; `Map(WRITE_DISCARD)`, `NO_OVERWRITE`, dynamic
  textures and uploads nothing reads do not. The fix, behind a switch, is below (2026-11-24).
- **MetalFX Spatial in `wsd`** (off, `OBELISK_WSD_UPSCALE=metalfx`): 1.5 ms of GPU a frame at full clock against 0.06 to 0.12 for today's draw (3 to
  5.6 ms when the GPU idles at a low clock), to 2940 x 1912 from 1323 x 860 or 1705 x 1109; 1.2 dB nearer to the frame drawn at full size than
  macOS's stretch, text and thin lines visibly sharper. It makes no frame faster.
- **Presented as it arrives** (off, `OBELISK_WSD_PRESENT=arrival`): 26.6 instead of 39.6 ms from the client's stamp to the screen at 40 frames a
  second, 32.2 instead of 45.6 at 30, the same even cadence. `presentDrawable:atTime:`, two drawables and no display sync are worse or uneven.
- **One more place in the guest's display queue** changes nothing where the GPU always has work; with a slow CPU frame in four it gives 45.7
  instead of 39.8 frames a second and costs 12 to 18 ms of latency. Only the guest's driver can give it.
- **`spindump` was macOS writing a disk-writes report of its own Launch Services** (`lsd`, over 2 GB in ten hours), not a hang report: each start
  rewrote the game's app bundle, which macOS registers again by saving a 12 MB database; a day of some 180 test starts crossed the limit
  during the 45 % start. `launch.sh` of the branch leaves an unchanged bundle alone (measured: 0 instead of 12 MB a start); it reaches
  players with the next obelisk-cli made from it.
- **"VRChat was not frontmost all along" was true**: Safari was in front in every look (`wsd`: `app_active=0`), the game shown over it. macOS gives
  the front to no program that the active app neither started nor yields to. The timeline's own bench (voile-lkd `tools/timeline/bench.py`)
  brings the game's app to the front and names what was there instead; `tools/mac-bench/bench.py` here is to take that over. Whether a
  start from Home gets the keyboard was not looked at.

### DXMT: a look at an event query flushes once (2026-11-24; no VRChat start)

`OBELISK_DXMT_QUERY_RESCUE=once` in the render server's environment (off unless set, in no release; DXMT `perf-timeline` `ed73857`). A `DONOTFLUSH`
look at an event or disjoint query after its 64th miss commits what has been recorded only while the query's marker was not handed to the
encoder, once; from there on it says `S_FALSE`. Flags 0, `Flush`, both `End`s, timestamps, occlusion and readbacks are as they were. The decision is
made under a small lock nothing waits under, against the last event handed over; the one commit under the device's lock, if it is free. Numbers,
conditions, what is not covered: voile-lkd's `tests/timeline/results/2026-11-24-query-rescue.md`.

| Owned client, frames a second, off / once | 100 % | 58 % | 45 % |
|---|---|---|---|
| nothing | 46.6 / 46.5 | 135.5 / 135.1 | 224.1 / 223.4 |
| one event query a frame | **35.6 / 46.5** | **128.7 / 135.4** | 226.1 / 223.9 |
| event query mid-scene | **34.6 / 39.5** | **101.2 / 113.7** | 187.2 / 185.8 |
| four timestamps; four occlusion queries | 35.2 / 35.3; 35.3 / 35.3 | 102.0 / 100.7; 100.4 / 100.4 | 169.0 / 167.9; 164.6 / 163.6 |
| event query, 16 ms of CPU between the draws | **29.6 / 45.6** | 61.2 / 61.2 | |

Commits for nothing 2.7 a frame and cuts of the target 2.6, with it 0 and 0.01; every picture's hash the same; 5.6 million randomised Ends without a hang.
An occlusion query whose draws span two targets, or that is in a command list, is counted wrong now and then by this DXMT, with the switch and without. Whether
VRChat gains is not known until it is started with and without.

### Next (not done)

1. One VRChat start per condition (58, 100, 45) with the timeline and the encoder times: the split of the fence wait, the GPU's
   idle gap, the encoder list. It needs the instrumented host files as an engine patch of their own (the CLI starts on released
   digests only) and the soak `AGENTS.md` asks for, though no file of the game's process changes.
2. The CPU power spent while the game plays (the ring's and the engine's spinning waits, N4N0's own window), since it is GPU clock.
3. The Low Power Mode session above, before any change to `wsd`.

## Results: 2026-11-23, 11:28 to 12:12, the first with the screen unlocked (obelisk-cli 0.1.8)

The same Mac (MacBook Air M3), world, avatar and settings as below, the screen **unlocked** and VRChat shown. Files
`2026-11-23_11-*.json` and `2026-11-23_12-06-100.json`. One start per row:

| Engine percent | Low Power Mode | Shown per second | Submitted per second | Batch interval, ms | Fence wait, ms | Record |
|---|---|---|---|---|---|---|
| 100 | on | 19.5 to 19.9 | 19.6 to 19.8 | 50.3 to 50.8 | 46.0 to 46.1 | `11-28-100` (partly beside a build) |
| 58 | on | 27.2 to 27.5 | 31.8 to 32.0 | 31.1 to 31.5 | 20.9 to 21.2 | `11-36-58` (beside a build at background priority) |
| 58 | off | 32.1 | 32.1 | n/a | 20.8 | `11-51-58`: from the engine's log over 15 minutes, the script's own file has no window (below) |
| 100 | off | 19.3 to 20.2 | 19.2 to 20.1 | 49.8 to 51.6 | 45.1 to 45.7 | `12-06-100` |

**No window of these files is scored**: the script read the frontmost app as None on macOS 27 (`lsappinfo`'s output changed;
fixed since), so every window says `valid: false` for that alone, though the screen was unlocked and frames were shown all
along; and the 11:51 start ran with a fixed script that took a running session for one that never started (fixed and
tested). Read them as counted observations of one start each, **neither paired nor repeated**: indicative. `11-05-58` was made
beside a compile with nothing shown, and counts for nothing; the 11:26 start ended by itself about 30 s in, before Easy
Anti-Cheat's first lines (the cause is unknown). Every start's VRChat log says "Failed creating world instance ... Bad
response data" and still joins; the script now checks that the place joined is the one asked for.

What they allow, and no more:

- **Submitted frames are the same with Low Power Mode on or off** (32 per second at 58 %, 19 to 20 at 100 %).
- **With Low Power Mode on, about 15 % of the frames submitted at 58 % were not shown** (27 of 32); none were lost at 100 %, nor
  at 58 % with it off.
- **58 % against 100 %: 1.6 times the frames shown** (32.1 against 19.3 to 20.2, Low Power Mode off).
- **The frame interval is mostly the fence wait**: 45 of 51 ms at 100 %, 21 of 31 ms at 58 %; the other 6 to 10 ms is
  everything else. Two points fit "9 ms + 36 ms x pixel share", as any two points fit a line: most of the wait scales with the
  pixels. How the wait splits between GPU execution and serial waiting in the render server is **not known**.
- **The rates with the screen locked (below) were about half of these** at 58 % (15.5 against 32 submitted): a locked screen is
  not a stand-in for production either. Nothing below is comparable with these numbers.

## Results: 2026-11-23, 05:11 to 05:55 (obelisk-cli 0.1.7 before its release, Orbiters - N4N0 0.3.9-dev.28 running)

MacBook Air M3 (4P+4E, 10 GPU cores, 16 GB), macOS 27, display 1470 x 956 points at 2x (2940 x 1912 drawn, 2560 x 1664 panel).
**Low Power Mode on while on AC** (`pmset -g custom`: AC `lowpowermode 1`), as in every earlier session: comparable on that
count. **The screen was locked for the whole session** (`CGSSessionScreenIsLocked` Yes, the bridge's `get_state` too): the window
server presented nothing (0 shown events, no Metal HUD line, `exited.frames` 0), so only production was measured, and Game Mode
(which needs the full-screen window shown) could not be compared. VRChat's home world `wrld_4432ea9b...`, a private instance of the
owner's each time, nobody else, his avatar, no input; VRChat's own graphics settings untouched. Seven starts, all arrived (38 to
52 s to `OnPlayerJoinComplete`), all quit by session; one start's numbers were lost to a bug of the script (it followed the
previous start's log, fixed), so six count:

| Start | Engine percent (drawn) | Batch interval, ms: median (p10 to p90 of 60-frame batches) | 1000 / median interval | Fence wait, ms | GPU busy % (device) | Game CPU % | Render server CPU % |
|---|---|---|---|---|---|---|---|
| 1 | 100 (2940 x 1912) | 118.0 (113.8 to 121.1) | 8.5 | 72.1 | 85 to 86 | n/a | n/a |
| 5 | 100 (2940 x 1912) | 107.5 (105.7 to 109.1) | 9.3 | 84.6 | 88 | 105 | 17 |
| 4 | 67 (1970 x 1281) | 72.8 (71.0 to 76.5) | 13.7 | 46.2 | 86 to 87 | 123 | 19 |
| 2 | 58 (1705 x 1109) | 64.8 (62.9 to 75.3) | 15.4 | 38.8 | 84 to 88 | 131 | 20 |
| 7 | 58 (1705 x 1109) | 64.1 (61.9 to 65.2) | 15.6 | 37.1 | 84 | 132 | 19 |
| 3 | 58, `-force-d3d11-bitblt-model` | 63.9 (62.2 to 69.5) | 15.7 | 39.4 | 79 to 81 | 122 | 20 |

The fourth column is **the reciprocal of the median batch interval, not a count of frames** (start 1's own file counted
7, 8 and 8 submitted per second in its windows where this column says 8.5). Every one of the six runs ended with zero
shown frames; start 6's numbers were lost to a bug of the script then (it followed the previous start's log) and no file
of it is scored. Spread between starts of one condition: 100 %: 107.5 and 118.0 (10 %); 58 %: 64.1 and 64.8 (1 %).

What these numbers allow (the fourth verification's verdicts), and no more:

- **Locked-screen guest production only.** With the screen locked the window server presented nothing: these are the
  rates at which the game *submitted* frames, not what anyone would see. 58 % against 100 % gave 1.66x to 1.84x the
  production (64 against 108 to 118 ms batch intervals), two starts per condition at most. Home's capped "100 %" on this
  panel is engine 87 %, which was never measured.
- **"40 ms + 73 ms x pixel share" is an empirical fit** of three points (40.03 + 72.73 x share), not a fixed cost, a
  floor or a bound; nothing below 58 % was measured there, so nothing is said about it. (2026-11-23, later, as the owner's session
  told it, in VRChat Home on this M3 Air: about 39 frames shown a second at 45 %, 31 at 58 %, 19 at 100 %; Home's default is 45 %
  from 0.3.9-dev.45.)
- **GPU busy 79 to 88.5 % and the render server at 17 to 20 % of a core are observations** (device-wide utilisation;
  start 1 has no CPU numbers), not a breakdown.
- **What the host wait (fence wait) is made of is unknown**: without GPU clocks, per-command-buffer times, queue latency
  or wake-up delays, "mostly GPU execution" is unsupported, as is its opposite (a busy GPU at lowered clocks fits too).
- **The bitblt test does not close overlap or thread priorities**: it changed the buffer count and format, did not show
  more work in flight, and low average CPU cannot exclude a latency-critical thread waiting to run. Overlap and QoS stay
  **open** (and the game's own threads are never given another priority: Easy Anti-Cheat).
- These rates are about half of the unlocked ones measured the same day (above): a locked screen lowers production itself
  (clocks, or the window server's behaviour: not known which). Not comparable with anything measured unlocked.
- The engine (0.1.7's fexdarwin-run) held: no group stop in seven starts, while wineserver attached the game's threads
  more than 4,096 times in each start's first minutes.

Not answered here: anything about presentation (nothing was presented), Game Mode, Low Power Mode off, shown frames, holds.
The unlocked results above begin to; a scored, paired session is still to make.
