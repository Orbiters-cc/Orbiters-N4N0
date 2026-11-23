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
once a second the console's lock state (`ioreg` `IOConsoleLocked`), the frontmost app (`lsappinfo`), GPU utilisation
(`ioreg`, device-wide) and the engine's threads' CPU (`ps -M`). It starts `obelisk-cli steam play 438100 [--render-percent N]
-- -screen-fullscreen 1 -window-mode borderless <link>`, waits (with deadlines) for the engine to say the game runs and for
`OnPlayerJoinComplete` in **this start's own** VRChat log (the one whose command line holds the link), waits `--warm`
seconds and then until 20 s pass without a frame held over 250 ms, records `--windows` windows of `--window` seconds
with nobody touching the game, takes 3 s stack samples only after them, and in every case (an arrival that never came, a
failed start, an interruption) quits by session (`steam quit --session --wait 60`, never forced) and writes its results
file with its `outcome`.

A window is **valid** only when, from its start to its end, the screen was unlocked, VRChat was the frontmost app and the
window server presented frames (positive presentation times); otherwise it is kept with `valid: false` and why, and never
scored. `--allow-locked` scores production alone behind a locked screen, and says `production_only` in each window. Rates
are counted, never reciprocal intervals: the presents (or shown events) between two lines of one swapchain generation
divided by the time between those two lines, the last line before the window included as its base, a count that went
back (a new swapchain) never subtracted across, a measured zero kept as 0.

`tools/mac-bench/test_bench.py` checks the scoring (boundary interval, generations, zero, validity).
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
  floor or a bound; nothing below 58 % was measured, so nothing is said about it.
- **GPU busy 79 to 88.5 % and the render server at 17 to 20 % of a core are observations** (device-wide utilisation;
  start 1 has no CPU numbers), not a breakdown.
- **What the host wait (fence wait) is made of is unknown**: without GPU clocks, per-command-buffer times, queue latency
  or wake-up delays, "mostly GPU execution" is unsupported, as is its opposite (a busy GPU at lowered clocks fits too).
- **The bitblt test does not close overlap or thread priorities**: it changed the buffer count and format, did not show
  more work in flight, and low average CPU cannot exclude a latency-critical thread waiting to run. Overlap and QoS stay
  **open** (and the game's own threads are never given another priority: Easy Anti-Cheat).
- These rates are far below the earlier unlocked sessions' (19 fps on average then, another world part of the time): a
  locked screen may lower clocks or change the window server's behaviour. Not comparable with the historical numbers.
- The engine (0.1.7's fexdarwin-run) held: no group stop in seven starts, while wineserver attached the game's threads
  more than 4,096 times in each start's first minutes.

Not answered: anything about presentation (nothing was presented), Game Mode, Low Power Mode off, shown frames, holds.
The session above is for them.
