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

1. Conditions written into the result by the script: `pmset -g custom` (Low Power Mode on AC and on battery), the power source,
   `pmset -g therm` at start and end, whether the screen was locked, the render percent the window server applied, the swapchain
   sizes. Keep fixed by hand: the same world and instance kind (a private instance of the owner's home world,
   `wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd`, nobody else), the owner's avatar as it is, VRChat's graphics settings untouched,
   video players off, no other foreground app, the Mac idle and cool for 5 minutes before (no fan), the screen **unlocked and
   the game in front** (only the owner can unlock it).
2. One condition per game start (the render percent is a launch option of the window server; a start costs the owner his Steam
   sign-in on the PC and must be 80 s or more after an early end). Alternate conditions across starts (A B A or A B B A) and
   treat each start as one sample.
3. The script starts `obelisk-cli steam play 438100 [--render-percent N] -- -screen-fullscreen 1 -window-mode borderless
   [--arg ...] <link>`, waits for `OnPlayerJoinComplete` in VRChat's log, waits `--warm` seconds more and then until 20 s pass
   without a shown frame held over 250 ms (loading and shader compilation settled), records `--windows` windows of `--window`
   seconds with nobody touching the game (the camera stays where the player spawned), and only after them takes 3 s stack
   samples of the render server and the game (intrusive sampling stays out of the scored windows). It samples GPU utilization
   (`ioreg`, IOAccelerator `Device Utilization %`) and per-thread CPU of the engine's processes (`ps -M`) once a second, quits by
   session (`steam quit --session --wait 60`, never forced) and writes one JSON file.

   ```sh
   python3 -I tools/mac-bench/bench.py --percent 58 --label "58" \
     --link 'vrchat://launch?ref=orbiters.cc&id=wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:<5 digits>~private(<owner usr_ id>)~region(eu)' \
     --warm 90 --window 60 --windows 3 --out docs/mac-bench-results/<date>-<label>.json
   ```
4. Report per condition the median of the per-batch production interval with its spread across batches and starts, the fence
   wait, GPU utilization, and (screen unlocked only) shown frames per second with p50/p90/p99 of the HUD intervals and the count
   of holds over 50 and 100 ms. A difference is a result only when it is larger than the spread between starts of one condition;
   with one start per condition, say "one sample".

## Results: 2026-11-23, 05:11 to 05:55 (obelisk-cli 0.1.7 before its release, Orbiters - N4N0 0.3.9-dev.28 running)

MacBook Air M3 (4P+4E, 10 GPU cores, 16 GB), macOS 27, display 1470 x 956 points at 2x (2940 x 1912 drawn, 2560 x 1664 panel).
**Low Power Mode on while on AC** (`pmset -g custom`: AC `lowpowermode 1`), as in every earlier session: comparable on that
count. **The screen was locked for the whole session** (`CGSSessionScreenIsLocked` Yes, the bridge's `get_state` too): the window
server presented nothing (0 shown events, no Metal HUD line, `exited.frames` 0), so only production was measured, and Game Mode
(which needs the full-screen window shown) could not be compared. VRChat's home world `wrld_4432ea9b...`, a private instance of the
owner's each time, nobody else, his avatar, no input; VRChat's own graphics settings untouched. Seven starts, all arrived (38 to
52 s to `OnPlayerJoinComplete`), all quit by session; one start's numbers were lost to a bug of the script (it followed the
previous start's log, fixed), so six count:

| Start | Engine percent (drawn) | Production interval, ms: median (p10 to p90 of 60-frame batches) | Frames submitted/s | Fence wait, ms | GPU busy % | Game CPU % | Render server CPU % |
|---|---|---|---|---|---|---|---|
| 1 | 100 (2940 x 1912) | 118.0 (113.8 to 121.1) | 8.5 | 72.1 | 85 to 86 | n/a | n/a |
| 5 | 100 (2940 x 1912) | 107.5 (105.7 to 109.1) | 9.3 | 84.6 | 88 | 105 | 17 |
| 4 | 67 (1970 x 1281) | 72.8 (71.0 to 76.5) | 13.7 | 46.2 | 86 to 87 | 123 | 19 |
| 2 | 58 (1705 x 1109) | 64.8 (62.9 to 75.3) | 15.4 | 38.8 | 84 to 88 | 131 | 20 |
| 7 | 58 (1705 x 1109) | 64.1 (61.9 to 65.2) | 15.6 | 37.1 | 84 | 132 | 19 |
| 3 | 58, `-force-d3d11-bitblt-model` | 63.9 (62.2 to 69.5) | 15.7 | 39.4 | 79 to 81 | 122 | 20 |

Spread between starts of one condition: 100 %: 107.5 and 118.0 (10 %); 58 %: 64.1 and 64.8 (1 %).

What it says, with that caution (one session, locked screen, two starts per condition at most):

- **Render scale pays.** 58 % against 100 %: 1.66x to 1.84x the frames produced (64 against 108 to 118 ms). With the three
  scales, the interval fits `40 ms + 73 ms x (share of the 2940 x 1912 pixels)` (67 % predicted 72.6 ms, measured 72.8): about
  two thirds of the 100 % frame scales with pixels, a floor of about 40 ms does not. Below 58 % little is left to win
  (25 % would give at most about 45 ms).
- **The host wait is GPU time.** The GPU stays 80 to 88 % busy at every scale, the render server uses about a fifth of one core
  (Neptune's decoder and DXMT's encoding together: about 12 to 20 ms of CPU per frame), the game about 1.0 to 1.3 cores of four.
  The fence wait (37 to 85 ms) is mostly the GPU finishing the frame. A 3 s sample of the render server after the windows shows
  its threads waiting (`npt_queue_thread` in `pthread_cond_wait`), the ring thread decoding and DXMT encoding in the rest.
- **"Nothing in a frame overlaps" is not what limits.** The bitblt model (one buffer, no frame-latency waitable: the analysis's
  test) took effect (`guest swapchain: created 1705x1109 fmt=29 buffers=1`, against 2 buffers with the flip model) and changed
  nothing (63.9 against 64.1 and 64.8 ms). While the GPU is this busy, letting frames overlap cannot buy much.
- **Not answered**: the half-refresh cadence and anything about presentation (nothing was presented), Game Mode, Low Power Mode
  off (only the owner can turn it off), shown frames, holds. These need the screen unlocked and the game in front.
- These rates are far below the earlier sessions' (19 fps on average then, 13 to 36 in stretches, unlocked, another world part of
  the time): a locked screen may lower the GPU's clocks or change the window server's behaviour. **Not comparable with the
  historical numbers**, only with each other.
- The engine (0.1.7's fexdarwin-run) held: no group stop in seven starts, while wineserver attached the game's threads more than
  4,096 times in each start's first minutes (`lkd: ptrace: attach 4096:`).

What this means for the larger changes (not started; for the next design round):

| Change | Expected value from this session |
|---|---|
| Frames overlapping (latency waitable, chunked commits) | Low while GPU-bound: the bitblt test moved nothing. Reconsider only if a scene shows an idle GPU |
| Presentation rewrite in wsd | Unknown: nothing was presented. Needs an unlocked run (the cadence question) |
| Upscaler (MetalFX spatial) | Worth it for picture quality at 58 % (the pixel share is large), but it costs GPU time, which is the limit: measure its cost first |
| Native-resolution UI | Same as the upscaler: a quality fix, paid in GPU time |
| Dynamic resolution | Plausible (two thirds of the frame scale with pixels), capped by the ~40 ms floor; needs a GPU-time target and the swapchain rebuilds measured |
| Thread priorities / QoS | Low: the render server is far from a full core; and not for the game's threads (Easy Anti-Cheat) |
