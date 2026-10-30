# N4N0 3D overlay latency: implementation and evidence

Date: 2026-10-30. Baseline: `b655149` / 0.3.4-dev.1. Development update: **0.3.4-dev.2**.

The main defect was in admission and publication of fresh pictures. The old reconstruction governor could keep
renewing a hold while the scene was late, preventing even the initial cube from appearing. Completed pictures then
waited for the next roughly 11 ms input turn. A picture predicted for its first display also grows stale over its
20/30/45 Hz update interval. These mechanisms explain why lowering reconstruction quality need not remove the lag.
They do not prove which mechanism dominated the user's headset session: SteamVR and VRChat were stopped here.

## Review and retained decisions

The requested **Claude Opus 5.5, maximum effort** review is preserved in
[the complete review](overlay-latency-opus-2026-10-30.md). Its input is the
[research and proposal brief](overlay-latency-review-2026-10-30.md). The reviewer read the implementation and baseline
artifacts without editing them. This implementation follows its blocking objections and core recommendations.

| Proposal | Decision and reason |
|---|---|
| A: overlay-specific admission | Retained. Observe new frame indices even during work/holds; one escalation per overlapping job; 500 ms recovery; 250 ms overload ceiling for affordable work. A 40% render-wall-cost duty limit can slow expensive jobs further. One job in flight; no catch-up queue. Reconstruction training is unchanged. |
| B: cadence | Retained at nominal 20/30/45 Hz using absolute deadlines. Rejected higher/half-refresh rates. Frame-rounded intervals were also not adopted: rounding across 72–144 Hz either exceeds those caps or lowers their average rate. Prediction uses display phase; admission does not claim exact frame locking. |
| C: completion publication | Retained on the existing overlay thread. Short high-resolution sleeps while work is pending, plus drains after menu/dashboard paints. No coarse channel timeout, process-wide timer change, input-thread migration or GPU wait on the input thread. Non-preemptible UI painting can still delay publication. |
| D: prediction and motion | Retained with separate wide/face/cube cost estimates; completion-to-display boundary with a 3 ms assumed latch lead; quarter-interval lifetime prediction capped at 12 ms, only after 100 ms stable motion. Acceleration, stops and reversals remove the extra lifetime term. Faster motion uses a 1 mm displacement gate, ordinary motion 2.5 mm; quiet tracking falls back at 3 mm. |
| E-lite: stereo buffers | Retained. Both calibrated eyes use the maximum of their required sizes while keeping their distinct frusta. Equal inputs are unchanged; unequal inputs avoid alternating image-buffer allocation. |
| E-full: extra size cache | Rejected for this update. Measured post-maintenance overhead was below the review's 0.5 ms threshold. Extra retained GPU memory is unjustified. |
| F: cube work | Separate cube prediction cost retained; maintenance waits for stillness and no overload. Staging twelve views across frames is deferred because shared masters/texture slots need a generation-aware design. The review's immediate post-switch burst and larger moving switch radius were not adopted without off-centre quality/live contention evidence. Cube switches remain a possible spike. |
| G: depth warp/new splat renderer | Deferred. New disocclusion, stereo and quality risks require separate image and headset validation. |
| Stereo sort-overflow recovery | Retained: the monotonically increasing overflow counter catches a left-eye overflow even if a later eye clears the redraw flag. |
| Diagnostics | Bounded local summaries written by a background thread, with nonblocking sends. Includes render/ready/submit timings per work kind, cadence, input turns, prediction horizons, overload levels and headset properties. |

The review corrected three claims in the brief. The old phase formula has a within-frame phase error, not a
systematic extra frame of prediction. The zero-overshoot cadence model was a worst case, not a Windows measurement.
A dropped scene frame has unknown cause, not necessarily a CPU cause. Temporal overlap with our work is still only
a conservative contention heuristic. The policy can do more work than before beside an already struggling scene,
including see-through mode; the bounded interval and wall-cost limit trade some scene headroom for visible progress.

The review's half-interval prediction was compared with zero and quarter-interval prediction. A quarter interval
gave a better worst case across the refresh/quality matrix, so it is the default. No old-policy product switch was
added: baseline code exists only in experiment harnesses, consistent with the project's no-compatibility requirement.

## Measurement boundaries

Hardware: NVIDIA RTX 4080, driver 616.92. Public VRChat Home PC tree, 786,432 leaf splats,
SHA-256 `d0010f58e976dd4c1ad4c13255446e0db95dca7264b33ba795e2968d7c4b5705`.
The installed app and Oculus service remained running. No user app was restarted and no headset session was started.

There are three distinct kinds of evidence: real Windows clock/worker scheduling, real offscreen GPU rendering and
synthetic GPU contention, and a deterministic motion/prediction model. None measures actual motion-to-photon latency,
SteamVR's overlay latch, live VRChat frame loss, or streaming-driver behaviour. Render wall time includes CPU work,
queue scheduling and GPU waits; it is not a GPU timestamp. The synthetic game separately uses GPU timestamps.

## Image and memory checks

Two independent baseline runs and two implementation runs produced **12/12 byte-identical capture PNGs** each,
including stereo, turned views and stale-position cases. Per-face textures were also compared separately. The
D3D12-to-D3D11 shared-texture check matched all **8,520,192 bytes**. Reported DXGI process memory was **515 MB**
in baseline and implementation runs. The normal symmetric-eye renderer has no demonstrated throughput change;
its new benefit comes from admission, prediction and publication.

With deliberately asymmetric calibrated-style tangents, common dimensions preserve normalized projection in a unit
test. Offscreen image quality changed by **−0.03 dB** for the affected wide left eye, within the review's −0.05 dB
limit; the right eye's PSNR was unchanged. This is not a claim of byte equality for resampled unequal eyes.

The maintenance-then-wide experiment repeatedly draws a face, times the following wide batch, then times a steady
wide batch. Initial symmetric runs showed 0.24–0.26 ms median overhead; final runs showed **0.36–0.42 ms**.
Final asymmetric common-size runs showed **0.44–0.50 ms** (rounded). There is no repeated result above the review's
0.5 ms threshold, so a second renderer-image cache was not retained.

Final asymmetric-eye A/B then B/A runs gave the following wide-batch medians. Light is within the run-to-run noise;
Balanced and Sharp show a consistent benefit. This is an artificial calibration fixture, not a measured property
of the user's headset.

| Quality | Independent sizes A / B | Common sizes A / B |
|---|---:|---:|
| Light | 5.03 / 5.50 ms | 5.15 / 5.42 ms |
| Balanced | 7.38 / 8.55 ms | 6.25 / 6.18 ms |
| Sharp | 12.26 / 12.08 ms | 8.10 / 8.02 ms |

## Real-clock scheduling and real-GPU motion

The windowless timing harness uses a 6 ms sleep worker, the production sleep-slice/pacer code and an 11 ms input
loop. It compares the old elapsed-time admission/polling policy with the new one, A/B then B/A, three seconds per
case. It has no SteamVR calls. Its optional 8 ms UI paint happens once at the start of an eligible input turn, as
in the real loop; an earlier harness draft incorrectly allowed painting inside service slices and was discarded.

| No-paint result, mean of two runs | Baseline | Current |
|---|---:|---:|
| Ready-picture publication delay, p50 across qualities | 4.60–4.99 ms | 0.75–0.96 ms |
| Ready-picture publication delay, p95 | 5.04–5.10 ms | 1.50–1.51 ms |
| Light / Balanced / Sharp observed admissions | 18.0 / 27.8 / 38.6 per second | 20.3 / 30.3 / 45.3 per second |

Short-window admission counts include the initial job, explaining the approximately 0.3/s above nominal. They do
not mean the production caps were raised. Current input-turn p99 stayed below 12 ms in these cases. With painting,
median publication delay was 0.98–1.35 ms versus 4.80–4.94 ms, but current p95 was **5.43–5.83 ms**, slightly worse
than baseline's 5.05–5.15 ms: a completed picture can be blocked by one non-preemptible paint. This tail remains.
This is not the full P-core-load/idle-CPU acceptance gate or actual `SetOverlayTexture` timing.

The twelve-second GPU trajectory now uses production motion gates and wall-cost pacing, with synchronous draw
completion. Light/Balanced/Sharp delivered **16.0 / 27.6 / 28.0 batches/s**, versus the old harness's
16.0 / 18.5 / 18.0. Render wall shares were **32% / 33% / 37%** versus 26% / 22% / 26%. There is deliberately more
work when motion needs it, bounded by measured cost; lower quality did not consistently mean faster completion in
these bursty workloads. These cross-run figures are descriptive, not a controlled end-to-end latency comparison.
The still control rendered its initial wide update and then no additional batches for the remainder of five seconds.

## Synthetic game contention

`gpu-lab` now records completion time from each paced frame's scheduled start, including queue and CPU scheduling
delay. A deadline miss is completion after 11.11 ms at 90 Hz, rather than a long wall-to-wall interval. Its CSV can
feed synthetic dropped-frame events into the production pacer in the offscreen renderer. File I/O is confined to
this experiment; the production overlay still reads OpenVR timing data and writes diagnostics off-thread.

Each case runs 16 seconds; the overlay runs for 12 seconds after a two-second game warm-up. Analysis uses game
seconds 5–13, excluding setup/teardown. Order is alone/old/new, then new/old/alone. The old executable's load harness
does **not** contain the old live governor, so this compares against its unthrottled workload, not the old live
overlay's potentially complete starvation. This distinction prevents a false claim that VRChat is now guaranteed
to lose fewer frames than the previous production policy.

| Synthetic workload | Alone deadline misses A / B | Old load harness | Current, feedback enabled |
|---|---:|---:|---:|
| Moderate, roughly 4 ms GPU median | 0% / 0% | 0% / 0% | 0% / 0% |
| Near budget, 1,600 shader iterations | 1.25% / 2.78% | 34.26% / 52.29% | 9.03% / 6.94% |
| Saturated, roughly 10 ms GPU median | 73.75% / 65.14% | 100% / 100% | 93.89% / 100% |

Moderate load: the current overlay delivered **28.42/28.44 batches/s**, versus **18.40/18.31**, with zero misses
across 1,440 analyzed game frames per policy. Game completion p99 was 8.27/8.53 ms with the current overlay,
versus 8.53/8.35 ms with the old harness and 6.12/6.14 ms alone.

At saturation the pacer reached level 4 / 250 ms and stayed live at **4.16 batches/s**, with **9% render-wall
share** versus the old harness's 38–39%. This is useful overload control, not a no-impact result: the game is
already missing most deadlines alone and the overlay adds load. A paced workload retaining every scheduled
deadline accumulates backlog once overloaded, so its large ready-time tails are not SteamVR frame latency.
The review's universal **+0.3 percentage-point** game-miss gate is **not established**. Liveness has an unavoidable
cost on a saturated GPU, and the live SteamVR/VRChat tradeoff remains a release limitation.

The additional near-budget run also reached the 250 ms floor at 4.17 overlay batches/s. Its 6.94–9.03% game-miss
rate was much better than the old unthrottled harness's 34.26–52.29%, but worse than the 1.25–2.78% game-alone rate.
Thus the no-impact gate is **failed against game-alone under near-budget load**, not merely unmeasured. Higher
capture rates, post-switch bursts and relaxed protection were rejected. Further improvement here requires shorter
GPU jobs or compositor-aware scheduling verified with the actual headset, rather than a more aggressive cap.

## Prediction model

`scripts/overlay-prediction-model.py` evaluates 72/80/90/120/144 Hz, 20/30/45 Hz quality caps, three linear speeds,
a head turn about a 10 cm neck pivot, stops/reversals, and walking with 60 ms cube switches. Assumptions include
6 ms ordinary render readiness, 11 ms display delay, 3 ms latch lead and 11.4 ms baseline turns. Extrapolation is
exact constant velocity; the model continues to refresh through stops and does not simulate GPU contention.

At 90 Hz/Balanced and 0.2 m/s, mean displayed-position error changes from **2.90 mm to 1.67 mm** with quarter-interval
prediction; p95 changes from **6.61 mm to 2.78 mm**. Across the 90 scenario combinations, mean improvement ranges
from **19.8% to 49.1%**. Half-interval prediction was better in many cases but its worst mean improvement was only
3.7%, at 72 Hz/Sharp. The review's universal 40% improvement gate is **not met**. These are modelled spatial errors,
not measured latency reductions or headset guarantees.

A separate sensitivity sweep varied baseline turns through 11.05/11.15/11.4/11.6 ms for linear, neck-turn and
stop/reversal cases at 72/90/144 Hz and all qualities. Mean gains ranged from 11.8% to 64.2%; p95 gains were at least
26.8%. This reinforces the decision to avoid a single universal percentage claim from a scheduler model.

## Acceptance and remaining validation

Validation: **186 desktop tests passed, 6 ignored; 9 OpenVR tests passed, 1 ignored**. The ignored checks require
live/external facilities; they were not enabled. The release programs compiled successfully with existing warnings.
The admission tests cover persistent lateness including the first cube, repeated history, recovery, unrelated/CPU/
async flags, frame-index wrap/restart/missing history, cadence and long-job duty. Geometry tests check both frusta.
The 6,000-sample bounded sinusoidal displacement fixture admits no translation updates with the quiet-motion flag
set; this isolates the gate, not the full tracked-velocity classifier. Real tracking noise and velocity estimates
still need a headset session. Cube costs do not change the wide cost estimate. The 40% wall-cost interval uses the
wide-job estimate; full cube replacements remain non-preemptible and are not an aggregate GPU-occupancy guarantee.
Prediction crossing the cube boundary is checked before planning mutates the cube transforms: even when the current
eye still fits, a replacement triggered by its predicted position uses the longer cube readiness estimate.

The review's exact frame-lock gate was deliberately replaced by the nominal-deadline policy described above.
The full P-core-load/idle-CPU gate, actual `SetOverlayTexture` latency, compositor-start distribution, prediction
residual sampling and live VRChat A/B frames remain unverified. Diagnostics do not yet include those last three
measurements. Walk-boost frame-sync behaviour and repeated moving cube switches also need live validation.

For a headset check, use Settings > About > Development environment and update to **0.3.4-dev.2**. Compare gentle
sideways movement, head turns, abrupt stops, walking across cube boundaries and an open menu at each quality, then
repeat beside a busy VRChat world. Local logs are `%LOCALAPPDATA%/Orbiters/logs/overlay-worlds.jsonl`, rotated with
one previous file around 1 MiB each. Logs stay on the computer; this change does not upload them.

## Reproduction and sources

Build with `cargo build --release -p orbiters-desktop`. Preserve the baseline executable before editing.
`scripts/overlay-latency-bench.py --help` documents windowless suites; pass absolute executable, capture and output
paths. Its contention suite also needs the separately built `scripts/gpu-lab` binary and the baseline executable.
The synthetic timing feed is benchmark-only and assumes 90 Hz. Raw local artifacts are under
`target/tmp/overlay-latency/`; [portable JSON evidence](measurements/overlay-latency-2026-10-30.json) contains the
reports, comparisons, model rows and sensitivity sweep. `gpu-lab --feedback` adds the fourth CSV column
(completion from scheduled start in milliseconds) and flushes each frame for the experiment's timing feed.

- Valve's [pose prediction documentation](https://github.com/ValveSoftware/openvr/wiki/IVRSystem::GetDeviceToAbsoluteTrackingPose)
  and [frame timing definitions](https://github.com/ValveSoftware/openvr/wiki/Compositor_FrameTiming) explain the
  display horizon and why GPU timing/drop flags need careful interpretation.
- [OpenVR releases](https://github.com/ValveSoftware/openvr/releases) and
  [the resolved WaitFrameSync issue](https://github.com/ValveSoftware/openvr/issues/1812) separate current API use
  from an older runtime defect. Blocking at a frame boundary still conflicts with publishing immediately when ready.
- [DesktopPlus's update policy](https://github.com/elvissteinjr/DesktopPlus/blob/master/docs/user_guide.md) supports
  separating texture updates from pointer responsiveness and avoiding stationary work.
- [Meta's compositor explanation](https://developers.meta.com/vr/documentation/unity/os-compositor/) explains why
  correcting rotation alone cannot recover stale translational parallax without depth.
- [VRSplat](https://arxiv.org/html/2505.10144v1), its
  [authors' implementation](https://github.com/Cekavis/VRSplat), and
  [XRgo](https://rsim.cs.illinois.edu/Pubs/25-MMSys-XRgo.pdf) informed the deferred renderer/depth-warp options.
  Their renderer and quality tradeoffs are not evidence of a free speedup for this overlay.
