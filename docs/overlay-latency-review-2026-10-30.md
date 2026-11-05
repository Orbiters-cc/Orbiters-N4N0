# 3D capture overlay latency: investigation and review brief

Date: 2026-10-30. Baseline: `b655149a1e94da78debebb9ddda7822464b712ce`, 0.3.4-dev.1.
This is a proposal, before implementation and independent review.

Historical review input: some diagnoses below were corrected by the
[Opus review](overlay-latency-opus-2026-10-30.md). Use the
[implementation results](overlay-latency-results-2026-10-30.md) for retained decisions and current claims.

## User report and constraints

The capture looks correct but follows movement sluggishly; raising reconstruction quality does not noticeably worsen it.
Preserve the appearance. Investigate deeply, obtain a Claude Opus 5.5 review, then implement retained changes and measure.
The user is away; defer questions. Publish through the repository's development update path when complete.

SteamVR and VRChat are not running during these measurements. The RTX 4080 is available, driver 616.92;
the installed Orbiters and Oculus service are running. Do not restart them or start a VR session.
GPU runs below use the real public VRChat Home capture, 786,432 leaf splats. They are offscreen experiments,
not headset motion-to-photon or live VRChat performance measurements.

## Pipeline and findings

`crates/orbiters-desktop/src/overlay/worlds/view.rs`: two world-locked cubes of six side-by-side stereo overlays.
An active cube is 3 m across. The second takes over after moving 45 cm from the centre. A full cube costs twelve
renders; normal batches render two wide views then resample them into the visible faces. SteamVR corrects rotation
by displaying world-locked quads, but has no scene depth to correct their stale translational parallax.

1. **Frame admission is independent of render cost and quantized by an unrelated timer.** `Quality::rate()` is
   20/30/45 Hz. `View::update` tests elapsed time since the last job. `overlay/mod.rs` sleeps to an 11 ms loop,
   using `WaitFrameSync` only for walk boost. Ideal continuous motion therefore produces 55/44/33 ms intervals
   (18.18/22.73/30.30 Hz), even if GPU work takes 5 ms. Real loop overhead changes these numbers.
2. **Persistent late scene frames can starve the overlay forever.** `vrchat_on_time()` borrows the reconstruction
   governor's late-frame verdict, with zero estimated cost. Every expiry of its 50 ms to 2 s exponential hold
   sees another late frame, starts another hold, and never admits a render. CPU-related dropped frames also trigger
   it, regardless of GPU headroom. `Verdict::Wait` is conversely ignored. This is a task-class mismatch: training
   may pause indefinitely, displayed head-tracked content may not.
3. **Late-frame identity is wrong for a sliding history.** The governor returns the newest frame index when *any*
   of the last four is late. A single bad frame moving through the window can be counted again (especially when
   scene frames arrive at half rate). Any change to the shared governor requires its own regressions, or keep
   overlay policy separate while sharing a small frame classification helper.
4. **Completed textures wait for the next main-loop poll.** `draw()` blocks the worker until completion, then sends
   `Answer::Rendered`. The overlay polls only once per loop; the picture may sit ready for nearly another 11 ms.
   `SetOverlayTexture` and the D3D11 flush happen only afterward.
5. **Prediction mixes costs and double-counts phase.** `batch_s` includes rendering plus polling delay and is updated
   for full-cube and maintenance jobs too. `ahead()` adds that duration to the entire time to the next vsync and
   vsync-to-photons, rather than finding the display boundary after readiness. Full-cube costs contaminate later
   wide-batch estimates. Exact compositor latch timing is not measurable without SteamVR; do not claim an exact
   photon deadline from the offline model.
6. **A 2.5 mm deadband suppresses slow parallax changes.** At 5 cm/s it alone permits 50 ms between updates.
   Rotating also translates the eyes. The 4 degree angular threshold is useful for coverage, not a substitute
   for updating translated eyes. Reduce only with a deliberate still-head/noise and GPU duty budget.
7. **Full-cube/maintenance work monopolizes the only in-flight slot.** New cubes draw all faces before swapping;
   maintenance can delay visible changes. Stage new-cube faces with cancellation/generation handling before
   considering an invasive new projection architecture.
8. **Potential allocation churn with unequal eye sizes.** `Gpu::ensure_image` keeps image buffers only for trainers;
   renderers discard them on every size change. `PlayCamera::wide` sizes each eye independently. Different eye FOVs,
   or switching between a face and a wide view, can churn buffers and bind groups. Our symmetric-eye baseline
   does not establish the user's headset has this problem. Measure asymmetric eyes before retaining a fix.
9. **Current timing labels are too strong.** `render::timed` is CPU wall time including queueing and GPU wait,
   not GPU timestamp duration. Distinguish render wall time, ready-to-submit delay, presentation cadence,
   prediction horizon, hold reason and actual headset timings in diagnostics.

## Baseline measurements

Original executable copied to `target/tmp/overlay-latency/baseline-desktop.exe` before any production edit.
Capture: `target/tmp/overlay-latency/home.pc`, fetched from the public slot 0 PC tree.
Two independent executions of `--world-view <out> <capture>`, reports in `baseline-a/report.json` and `baseline-b/report.json`.

| Quality | Stereo batch median, run A | Run B | Existing cap | Ideal 11 ms-loop cadence |
|---|---:|---:|---:|---:|
| Light | 4.72 ms | 5.10 ms | 20 Hz | 18.18 Hz |
| Balanced | 5.77 ms | 6.15 ms | 30 Hz | 22.73 Hz |
| Sharp | 8.09 ms | 8.60 ms | 45 Hz | 30.30 Hz |

Batch numbers are wall-clock completion times; twelve moving views per quality, not isolated GPU timestamps.
Both runs: D3D12-to-D3D11 readback of 8,520,192 bytes identical; process GPU memory 515 MB. Wide-view composition
PSNR versus direct eye render: 39.67/39.92 dB left/right. All ten image metrics agree between baselines to 0.01 dB.
Stale-position composition: 32.67 dB at 2 mm, 30.67 at 5 mm, 26.32 at 20 mm, 23.28 at 50 mm.
`scheduler-baseline.json` is a deterministic model of the current timing branches, not a SteamVR trace.
Its continuous-late-frame scenario admits no work in 20 seconds, with holds growing to 2 seconds.

Additional baseline collected during review: 12-second existing `--load` trajectories (turn +/-35 degrees, sway
+/-4 cm) gave Light 15.98 batches/s, median/p95 completion 16.87/32.30 ms; Balanced 18.49 batches/s,
11.16/23.38 ms; Sharp 17.97 batches/s, 15.59/20.39 ms. These include different frusta, cold/bursty GPU behavior,
and movement thresholds. The short burst benchmark above must NOT alone set an aggressive new rate. The existing
`--load` harness directly draws at its own timer and does not exercise `View`'s main-thread admission or governor.

## Candidate changes for review (not approved merely by appearing here)

A. **Retain provisionally: separate an interactive overlay pacing policy from background training.** Deduplicate
actual late events. Account for GPU versus CPU bottlenecks. Bound starvation with a deliberately rate-limited
probe, not unconditional high-frequency rendering under overload. Ignore stale or invalid timing history safely.
Use elapsed-time recovery, not recovery on every loop iteration. Test persistent GPU overload, CPU-bound scenes,
one sliding late event, frame-index wrap/restart, missing timings and slow GPU jobs.

B. **Retain provisionally: synchronize admission to headset frames and decouple resolution from responsiveness.**
Use the existing `WaitFrameSync` wrapper while 3D content is active, with bounded timeout/no-spin guard. Replace
elapsed-since-job rounding with stable deadlines or integer display-frame cadence. Keep one job in flight, never
catch up by queueing old poses. Candidate budget: bounded fraction of GPU time from measured batch cost, with
headset-rate ceiling and slower overload cadence. Do not simply change every quality to uncapped 90 Hz.
Review whether initial improvement should retain nominal caps or target e.g. half refresh with measured headroom.

C. **Retain provisionally: publish completion promptly.** Wake the main loop when a worker answer is ready, bounded
by its input/event deadline, or separate submission from expensive panel painting. Keep SteamVR texture/device
ownership on its current thread; avoid adding a second thread issuing graphics overlay calls. Do not remove
the existing shared fence, double buffers or D3D11 post-handover flush without direct evidence.

D. **Retain provisionally: prediction and movement freshness.** Keep wide render-cost estimates separate from
initialization/maintenance. Predict to a bounded display boundary after readiness, validate constant velocity,
stop/reversal and long-job cases in a synthetic trajectory. Use an inexpensive movement threshold plus a maximum
moving-picture age, while staying idle under tracking noise. Capture actual pose and age diagnostics for later VR use.

E. **Measure before retaining: bounded renderer image-buffer reuse** for the two eye sizes plus face size, opt-in
for this fixed-size multiview workload; avoid increasing ordinary desktop/training memory unexpectedly. Verify
identical pixels and memory bounds; compare symmetric and asymmetric eyes, and maintenance switching.

F. **Consider if it fits safely: stage cube fills and prioritize visible work.** More complex because master images,
texture slots and recenter/quality changes are shared. Explicit generation/cancellation is necessary. Preserve
atomic cube appearance; never show uninitialized faces. Estimate whether occasional stalls justify the scope.

G. **Defer unless evidence changes:** replacing cubes with projection overlays, own depth reprojection or temporal
frame generation, global CPU/radial shared sort, a new quad renderer, tree LOD, foveation. They may improve larger
scenes but can change image quality, occlusion/stereo ordering, seams or coverage. No evidence yet they are needed
to fix the measured scheduling defects. A separate standalone scene app would interrupt VRChat and is unsuitable.

## Research evidence and implications

- [Valve OpenVR release notes](https://github.com/ValveSoftware/openvr/releases): `WaitFrameSync` exists specifically
  for overlay pacing; timeout required when signals are missed. Already wrapped locally.
- [Valve pose prediction documentation](https://github.com/ValveSoftware/openvr/wiki/IVRSystem::GetDeviceToAbsoluteTrackingPose):
  predict to photons using frame phase and display delay; its simple example assumes no extra buffered frames.
- [Valve frame timing definitions](https://github.com/ValveSoftware/openvr/wiki/Compositor_FrameTiming): dropped,
  mispresented and GPU/CPU reprojection are distinct; async-active flag alone is not evidence of a missed frame.
  GPU timing can include other processes' scheduling, so it is not a clean causal measurement of our own GPU cost.
- [fpsVR author's WaitFrameSync report](https://github.com/ValveSoftware/openvr/issues/1812): the 2024 degradation
  was reported fixed in SteamVR 2.3.2. Do not reject frame sync based on that old fixed issue.
- [SteamVR developer on OpenXR and overlay sync](https://steamcommunity.com/app/250820/discussions/3/600786717156353525/):
  same-process OpenXR/OpenVR mixing is unsupported, and an active scene process cannot block on overlay sync.
  Our standalone overlay process is different; retain a no-spin guard for unexpected immediate returns.
- [Unity overlay pacing discussion](https://discussions.unity.com/t/forced-uncapped-framerate-on-overlay-application/931609):
  first-person report of smoother pacing with WaitFrameSync; corroboration, not benchmark evidence for N4N0.
- [Valve Unity XR release notes](https://github.com/ValveSoftware/unity-xr-plugin/releases): v1.2.3 introduced overlay
  WaitFrameSync and one-frame pose prediction. Current raw master display file differs from the linked old excerpt;
  do not copy offsets or implementation blindly.
- [Desktop+ documentation](https://github.com/elvissteinjr/DesktopPlus/blob/master/docs/user_guide.md): variable
  updates and per-overlay limits avoid work when content is unchanged. Its separate pointer-latency tradeoff
  reinforces measuring input/pose age separately from texture complexity; it is primarily a 2D overlay.
- [Meta compositor explanation](https://developers.meta.com/vr/documentation/unity/os-compositor/): rotation
  reprojection cannot correct arbitrary translational parallax without depth, including eye translation on head turns.
- [VRSplat paper](https://arxiv.org/html/2505.10144v1) and [authors' implementation](https://github.com/Cekavis/VRSplat):
  optimized projection, sorting and single-pass foveation address VR artifact/performance tradeoffs. Its 72+ Hz
  demonstration is on an RTX 4090 with its own model/rendering setup, not a drop-in estimate for ours.
- [XRgo paper](https://rsim.cs.illinois.edu/Pubs/25-MMSys-XRgo.pdf): translational reprojection exposes disoccluded
  pixels; filling them is a quality/cost problem. This argues against adding a quick depth warp to already-good images.

## Proposed validation

Pure production-policy regressions and deterministic trajectories at 72/80/90/120/144 Hz; slow jobs and overload;
report submission intervals and modelled pose age separately from real motion-to-photon. Repeated offscreen real
capture runs, same quality and camera sequence, with independent baseline repeats and image comparisons.
Measure buffer churn with unequal eye sizes if pursuing E. Measure concurrent paced synthetic game frames and
uncapped contention, including a still-head control. Persist bounded live diagnostic summaries for later headset
validation. Required final caveat: actual SteamVR latch timing and VRChat frame impact remain unverified while stopped.

## Questions for the independent reviewer

Challenge the diagnosis, priority, admission policy and prediction math. Which A-F changes should ship together?
Propose concrete conservative constants/algorithms and identify regressions likely to escape offline tests.
Give blocking objections high priority. The parent will implement, measure and respond to every substantive finding.
