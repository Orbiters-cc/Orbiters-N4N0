# Independent review: 3D capture overlay latency (brief of 2026-10-30, baseline `b655149`)

**What I read.** The brief; `overlay/worlds/{view,render,stereo,interop,shots,mod}.rs`; `overlay/{mod,menu,frames}.rs`; `orbiters-reconstruct/src/{governor,gpu}.rs`; the `orbiters-openvr` wrappers; `docs/world-view-overlay.md`; `scripts/gpu-lab`; and every artifact in `target/tmp/overlay-latency/`. Nothing was run or edited, and the tree is unchanged.

**Limits.** Every conclusion here is offline. None of it establishes SteamVR's latch timing, motion-to-photon latency, or the impact on VRChat. Paths below are relative to `H:/metaverse/ORBITERS/Orbiters-N4N0`.

## 0. Verdict

- **The direction is right.** The defects are in scheduling, not in rendering or appearance.
- **Three parts of the diagnosis are wrong or overstated.** They change the priorities: D1, D2 and D3 below.
- **The brief leaves out evidence that limits B.** This includes its own load and contention runs and the design doc's 50–80 ms cube-switch cost.
- **B as proposed conflicts with C.** Pacing the loop on `WaitFrameSync` and publishing only at frame boundaries gives back a frame of latency.
- **Plan.**
  - Ship A, C and D, plus E-lite and diagnostics, as the core.
  - Ship B only at the nominal caps, and only together with A and C.
  - No half refresh this round.
  - F: defer staging and take the cheap mitigations.
  - G: defer, as you proposed.

---

## 1. Corrections to the diagnosis

**D1 (High). "Prediction double-counts phase" is wrong as a systematic claim. The real systematic error is somewhere else.**

`ahead()` at `view.rs:596-601` returns `batch_s + (frame − since) + photons`. Compare it with the correct target, "the first latchable vsync after the job is ready":
- If the job would make the next vsync, the formula overshoots by `B`.
- Otherwise it undershoots by `frame − B`.

Averaged over a uniform vsync phase, that error is **zero-mean and within ±1 frame**: random jitter of about ±5 ms at 90 Hz, not lag.

The systematic error is missing from the brief. Each picture stays on screen for a whole admission interval `D` (33–55 ms) but is predicted to its *first* photon. Under motion, the error therefore ramps from 0 to `v·D`, with mean `v·D/2` (17–27 ms of motion). Predicting toward the middle of the display interval roughly halves both the mean and the peak error at zero GPU cost. It is the largest free win available.

**D2 (High). The cadence numbers come from a zero-overshoot model, not a baseline.**

- Admission compares `last_job.elapsed()` (`view.rs:524`) against a timestamp taken at a varying point inside the turn.
- A turn is `sleep(TURN − elapsed)` (`mod.rs:770`). With Rust 1.99's high-resolution `thread::sleep`, the period is therefore 11 ms plus overshoot plus jitter in the work done before `View::update`.
- Balanced admits after 3 turns whenever `3·period + jitter ≥ 33.33 ms`, i.e. whenever overshoot is at least about 0.11 ms. Sharp does the same at 2 turns.
- Realistic overshoot (0.05–0.5 ms) puts Balanced near 29–30 Hz and Sharp near 43–45 Hz, with occasional extra 11 ms gaps. The 22.7 / 30.3 Hz figures are the worst case.

So B's likely real gain is **regularity**, plus better behaviour in turns where the menu paints, rather than +32 % rate. The source of the scheduler model is not in the repo.

Before claiming anything for B, measure the real turn-period distribution: production sleep code, in a windowless child process, on the user's PC.

**D3 (Medium). Dropped frames are not "CPU-related".**

`num_dropped_frames > 0` means the compositor missed vsync, cause unknown. CPU reprojection (`0x01`) is already ignored (`governor.rs:43`). The compositor normally runs on a high-priority GPU context, so our work rarely causes drops, and streaming drivers report drops more often. Count drops only when they are attributable to our work (§4.1).

**D4 (Medium). The 4-entry window both double-counts and misses late frames.**

The view reads 4 timings, and only when admission is otherwise allowed (`view.rs:524-527, 556`).
- **Missed events (under-reaction):** late frames that arrive and scroll out while a job is in flight or a hold is active are never seen.
- **Double counts (over-reaction):** a late frame still in the window after the first 50 ms hold triggers another back-off. At 72 Hz, 4 app frames span 55.6 ms; at half rate they span about 89 ms.

`FrameSampler` (`frames.rs:39-55`) already does this correctly: it reads many entries and processes each index once.

**D5 (High, sharpens finding 2). The capture can never appear.**

In `Phase::Loading`, the first cube is also gated by `vrchat_on_time` (`view.rs:521-529`). A scene that is persistently GPU-late leaves the panel at "Getting your graphics card ready" forever. This is worse than "parallax lags".

---

## 2. Overlooked causes and omitted evidence

| # | Sev. | Finding | Evidence |
|---|---|---|---|
| E1 | High | **The cost to a game is already measured and argues against raising cadence blindly.** | A GPU-bound stand-in lost 6.6 / 9.4 / 14 % of its frames at Light / Balanced / Sharp, with batches at only 16–18 a second (`docs/world-view-overlay.md:107-124`). The `baseline-load-*` and `baseline-contention` logs show batch wall time doubling under contention: medians 8.5–16.9 ms against 4.7–8.6 ms isolated. Paced stand-in: p99 wall 12.66 → 14.27 ms, max 13.1 → 16.8 ms, GPU time unchanged, so our work delays its start. |
| E1b | Med | **The `--load` harness is not representative.** | Its cadence is gated by `MOVED_M`/`TURNED` (`shots.rs:344-349`). Eyes rotate about the head centre with no neck pivot (`shots.rs:391-397`), which understates real eye speed. It does not model the 11 ms loop at all (`shots.rs:336`). `gpu_busy_share` is a share of wall time (`shots.rs:357, 370`). |
| E2 | High | **A cube switch stalls parallax for 50–80 ms** (`world-view-overlay.md:104-105`), which the brief omits. | While walking, a switch happens every ~0.45 m of travel, about every 0.9 s at 0.5 m/s. The new faces are predicted with the wide-batch EMA (~12–15 ms) although they appear 50–80 ms later. The cube's sample then inflates `batch_s` (`view.rs:713-715`): +9.6, +7.7, +6.1 ms over the next batches. |
| E3 | High | **The headset is probably not an Index.** | Memory notes from an earlier session say the default mic is Virtual Desktop's, and the brief says the Oculus service is running. That points to a Quest streamed through Virtual Desktop or Link. Expect refresh 72/80/90/120 Hz, driver-specific `SECONDS_FROM_VSYNC_TO_PHOTONS` (clamped at 50 ms, `view.rs:599`), half-rate SSW/ASW frame timings, and spurious drops. Tune nothing to Index assumptions, and log the driver and model. |
| E4 | Med-High | **Windows may schedule the overlay process as background work.** | It is spawned windowless (`service.rs:185-190`) at normal priority with no QoS opt-out, on a likely hybrid CPU (UHD 770). gpu-lab's `like_a_game` (`scripts/gpu-lab/src/main.rs:407-425`) opts out of EcoQoS for exactly this reason. Offline runs from a foreground console will not reproduce it. Also: timeouts in `mpsc::recv_timeout`, `Condvar` and `WaitForSingleObject` are tick-based (up to ~15.6 ms late unless this process raises its timer resolution). Only `thread::sleep` uses a high-resolution timer. |
| E5 | Med | **Menu and panel painting share the loop.** | `worlds.update` runs before the menu paints (`menu.rs:196` vs `337-385`) and before the dashboard paints (`mod.rs:727-758`). CPU painting takes ~8 ms. While the panel is open — exactly when the user is judging quality — turns reach ~19 ms, and publication and admission slip by that much. |
| E6 | Med | **Protection ignores scene context.** | The hold also fires during VRChat's loading screens, while the dashboard is open, or with no scene app. `mod.rs:654-659` already computes the scene state; the view does not use it. |
| E7 | Med | **Image buffers churn for every user, not only asymmetric headsets.** | Every maintenance job (`view.rs:660-680`, run only while the head is still) is followed by a wide batch. That batch reallocates the image buffers and clears the *entire* bind cache (`gpu.rs:1386-1396, 1058-1060`), so the motion-onset update pays the cost. Factory-calibrated headsets can also round to a 1 px width difference between eyes (`stereo.rs:326`), which reallocates twice per batch. The baseline mirrored the tangents exactly (`shots.rs:385-388`) and timed only consecutive wide batches (`shots.rs:196-221`), so it cannot see either case. |
| E8 | Low | **Shared renderer: one `redraw` flag for both eyes.** | It is set at `gpu.rs:1558` and cleared at `1731-1733` by a later waited step. A left-eye overflow found when the right-eye step starts gets cleared by that step and is never redrawn. Rare. |
| E9 | Info | **`REACH_M` limits sampling quality, not correctness.** | `Cube::camera` (`stereo.rs:224-238`) is an exact projection for any eye inside the 1.5 m cube. This opens the F alternative in §4.6. |
| E10 | Info | **Face distance is SteamVR's free depth proxy.** | Stale-parallax error ≈ `δ·|1/z − 1/d|`. The down face sits about 0.1 m above the floor, which is why the floor already behaves. Worth an offline `HALF_M` sweep with the existing `moved-*mm` metric; not for this release. |

Two smaller points:
- The measured cost of staleness supports E1: 2 mm costs −0.76 dB, 5 mm −2.8 dB, 20 mm −7.1 dB (`report.json`). The 2.5 mm deadband is therefore a small contributor; interval × speed dominates.
- The design doc's wide-batch PSNR (40.9 dB) no longer matches the current baseline (39.67 dB). Refresh the table.

---

## 3. Decisions on A–G

| Change | Decision | Conditions |
|---|---|---|
| **A** | **Retain. Mandatory; ship first.** | A new overlay-only pacer as a pure module (§4.1). Leave `governor.rs` untouched this release; share at most a classification helper. The trainer has the same dedupe bug (`governor.rs:42-46, 98-103`): file it separately with its own regressions. |
| **B** | **Retain, modified.** | Frame-counter cadence at the **nominal caps**, a precise wake, and a duty ceiling. No half refresh. Never publish only at `WaitFrameSync` boundaries. Never ship without A and C. Do not raise Light's rate: `Quality::Performance` is the weak-GPU choice. |
| **C** | **Retain.** | Same thread. Sleep in high-resolution slices of 1 ms or less while a job is in flight, and add publish points after each paint (§4.2). Keep the fence, the double buffers and the flush. |
| **D** | **Retain.** | Per-kind readiness EMAs measured on the render thread; the display boundary after readiness; a `+β·D` term; a velocity-gated soft deadband; a residual diagnostic. |
| **E** | **Split.** | **E-lite:** one common wide size for both eyes. Retain: zero risk, byte-identical for mirrored eyes. **E-full:** an opt-in renderer cache of 2 image sizes. Measure first with an interleaved benchmark. |
| **F** | **Defer staging.** | Masters and the 12 slots are per face, not per cube (`render.rs:62, 175-193`; `view.rs:635`). Staging needs about +100 MB at Balanced and +200 MB at Sharp, plus generation handling. Take the §4.6 mitigations instead. |
| **G** | **Reject for this release.** | Optional offline research only: the proxy-depth sweep (E10), and lowering the process GPU scheduling priority class. |

---

## 4. Concrete algorithms and constants

### 4.1 Pacer (A, with B's cadence)

A pure state machine with injected time, vsync and timings:

```
TIMINGS_READ = 16      // read every turn and every wake; process each frame_index once
FLOOR        = 250 ms  // liveness: max gap between admissions while a batch is due (first cube included)
RECOVER      = 500 ms  // elapsed time per level down, with no attributable late event
ATTRIB_AFTER = 2       // app frames after a job's completion still count as ours
DUTY_MAX     = 0.40    // EMA of wide-job wall time × admission rate (a ceiling, not a target)
N_nominal(q, hz) = max(1, round(hz / q.rate()))
                 // 90 Hz: 5/3/2 · 72: 4/2/2 · 80: 4/3/2 · 120: 6/4/3 · 144: 7/5/3

observe(timings, protect = scene_focus_process != 0 && !is_scene_loading()):
  for t newer than last_seen:          // wrapping compare; a jump back >1000 is a restart: resync
    late = (t.reprojection_flags & 0x02) || t.num_dropped_frames > 0   // ignore Cpu/Async/Motion/Throttle bits
    if !late: continue
    if !protect: count ignored; continue
    if a recent job j has idx_admit < t.frame_index <= idx_done + ATTRIB_AFTER (in flight: open)
       and j has not escalated yet:
         level = min(level + 1, max_level); j.escalated = true; last_attr = now
    else: count unattributed           // logged, never escalates
  if level > 0 and now − max(last_attr, last_step) >= RECOVER: level -= 1; last_step = now

interval_frames = min(max(N_nominal << level, ceil(cost_wide_s · hz / DUTY_MAX)),
                      floor(FLOOR · hz))                       // max_level: first level reaching the floor
may_admit = !in_flight && vsync_counter − last_admit_counter >= interval_frames
            && (kind != Maintenance || (level == 0 && head_speed < 1 cm/s))
```

- **Frame counter.** `time_since_vsync` discards SteamVR's frame counter (`orbiters-openvr/src/lib.rs:497-502`). Return it from the wrapper.
- **Under persistent lateness** the pacer settles at about 4 Hz, roughly 2.4 % GPU, instead of 0 Hz.
- **Escalation** happens at most once per job, so one 50–80 ms cube job cannot climb three levels.

### 4.2 Wake and publish (C, and B's timing)

When the walk boost is not moving, replace `mod.rs:769-771` with:

```
let end = turn + TURN;
while now() < end {
    let wake = view.wake_at().map_or(end, |w| w.min(end)); // in flight: now + 1 ms; admission: its vsync + 0.5 ms
    std::thread::sleep(wake.saturating_duration_since(now()).max(500 µs)); // high-res; no recv_timeout/Condvar
    view.service(&overlay, &steamvr);  // try_recv → handed(); observe timings; admit if due
}
```

- Also call `view.publish()` after the menu paint and after the dashboard paint.
- Full turns stay at 11 ms, so input reads and paint caps are unchanged.
- **If you prefer `WaitFrameSync`:**
  - Admit when the frame counter is at least `last + n`.
  - Before the next wait, if the job's ready estimate is below `frame − L − elapsed`, wait for it in sleep slices of 1 ms or less and publish.
  - No-spin guard: if a wait returns sooner than half a frame after the previous return, sleep until `previous + frame`.
- **Walk boost:** keep the existing path, and call `service()` both before and after `WaitFrameSync`.

### 4.3 Prediction (D)

```
R  = ready_ewma[kind] + ready_absdev_ewma[kind]   // kinds: Wide / Face / Cube; ready_at stamped on the render thread
L  = latch lead: default 3 ms, clamped 1..6        // later: from FrameTiming.compositor_render_start_ms
t0 = frame − since_vsync
k  = max(0, ceil((R + L − t0) / frame))
h  = min(t0 + k·frame + P + min(β · interval_frames · frame, 12 ms), 100 ms)   // P: photons, clamped 0..50 ms as today
```

- Default `β = 0.5`. Choose among β = 0, 0.25 and 0.5 using the G-D1 trajectories.
- Keep the panel's "ms a batch" on Wide jobs only. Label it render time, not GPU time.

### 4.4 Freshness (D)

- **Hard trigger:** as today (2.5 mm, 4°, count change, redraw).
- **Soft trigger:** moved more than 1.0 mm, *and* head speed above 2 cm/s (from `Pose::velocity`), *and* the picture is at least one interval old. This adds no GPU work beyond the cap.
- **Still head:** below 1 cm/s and 3°/s, never soft-trigger. Maintenance is allowed only in this state.

### 4.5 E-lite

- Compute both eyes' wide cameras, then rebuild each at `(max(width), max(height))` with `fx = W/(r−l)`, `fy = H/(hi−lo)`, and `cx`/`cy` as today.
- This gives a common size for both eyes. When the sizes already match, the output is byte-identical.

### 4.6 Cube switches (F-lite)

- Give Cube jobs their own readiness EMA and prediction horizon.
- Right after a switch is published, allow one wide batch at `n/2` when the pacer is at level 0.
- **Experiment:** switch hysteresis. Switch at 0.45 m when the head is slow, and only at a hard 0.6 m while it moves or while `level > 0`. Measure sampling quality against off-centre distance with the harness before adopting it.

---

## 5. What ships together

- **Preferred: one release** with A, B (nominal caps), C, D, E-lite and diagnostics.
- **If you split it:** first A, C, D, E-lite and diagnostics. Together they add no GPU load. Then B.
- **Never:**
  - B without A: more GPU pressure on a governor that escalates to starvation.
  - B with frame-boundary-only publication: +1 frame.

## 6. Blocking objections, and regressions likely to escape offline tests

1. **[Blocking] No coarse-timeout waits** (`recv_timeout`, `Condvar`, `WaitForSingleObject`) in the loop. They stretch turns to about 16 ms, and the menu's double-press binding depends on every turn reading the controllers (`mod.rs:52-54`).
2. **[Blocking] Liveness must include the first cube**, and a test must assert it.
3. **[Blocking] No file I/O on the loop thread** for diagnostics. Batch them through the backend the way `backend.frames` does (`mod.rs:669-673`), or write from a background thread.
4. **[Blocking, for claims] No ms motion-to-photon claims.** Report cadence and modelled pose age only.
5. **Likely to escape offline tests:**
   - the windowless process's QoS and E-core placement;
   - streaming-driver frame-timing semantics (SSW half rate, drops, photons property);
   - the compositor latch phase and the CPU cost of `SetOverlayTexture`;
   - scene loading transitions;
   - walk-boost carries forcing constant cube switches;
   - `frame_ms` read once (`mod.rs:564`) if the refresh rate changes mid-session.

   Attribution can also make the overlay more aggressive than today in see-through mode. That is bounded by the duty ceiling; document it.

## 7. Acceptance gates (offline)

| Gate | Requirement |
|---|---|
| G-A1 | Every frame GPU-late for 20 s while moving: at least 3.8 admissions/s; max gap ≤ 260 ms plus one interval; first cube within 260 ms of load (today: 0). |
| G-A2 | One late frame seen across 16 reads, at full and half rate: exactly one level step; back to level 0 within `500 ms × levels`. |
| G-A3 | CPU-only, motion-smoothing, throttle or mispresent frames, unattributed lateness, and frames while not protected: no escalation. |
| G-A4 | Frame-index wrap, restart, `n = 0`, and `compositor()` errors: no stuck state; nominal cadence. A 40 ms job: never two in flight. |
| G-B1 | Uncontended: at least 99 % of intervals exactly `N_nominal` frames, none shorter, at 72/80/90/120/144 Hz. |
| G-B2 | Realized rate at least the **measured** baseline per quality (not the model). |
| G-B3 | Idle turn p50 ≤ 11.3 ms, p99 ≤ 13 ms (windowless child process, CPU load on the P-cores). Still-head CPU at most +0.5 % of a core. |
| G-C1 | Ready to `SetOverlayTexture`, with a fake overlay: p50 ≤ 1 ms, p99 ≤ 2 ms idle; at most one paint (~8 ms) while the panel paints. |
| G-D1 | Synthetic trajectories (0.05 / 0.2 / 0.5 m/s, yaw with a 10 cm neck pivot, stop, reversal, walking with switches): time-weighted mean displayed error at least 40 % below baseline and p95 at least 25 % below; stop/reversal p95 at most 10 % worse. |
| G-D2 | Horizons stay within ±2 ms of steady state after a switch. |
| G-D3 | Still head with 0.3 mm and 1 mm RMS noise: zero admissions in 60 s. |
| G-E1 | Identical inputs give byte-identical faces against `baseline-desktop.exe`, E-lite included. |
| G-E2 | Calibrated-style asymmetric tangents: equal sizes, PSNR at least baseline − 0.05 dB, no per-batch reallocation (instrument `ensure_image` / `forget_binds`). |
| G-E3 | Interleaved maintenance-then-wide benchmark: E-full only if the post-maintenance batch is more than 0.5 ms above steady-state p50. Memory at most +32 MB Balanced, +64 MB Sharp. |
| G-M1 | A–D: DXGI local memory unchanged (±5 MB). |
| G-K1 | Closed loop with gpu-lab paced at 90 Hz (4.5 ms and about 9.5 ms GPU), counting deadline misses against its own due time, with misses fed back as synthetic `FrameTiming`: misses at most baseline + 0.3 percentage points; overload settles at the floor. |
| G-K2 | Still-head control: zero GPU work. |

## 8. Diagnostics to ship (bounded 10 s summaries)

- Admission intervals in frames and ms.
- Level histogram and time spent at the floor.
- Late events split into attributed, unattributed and ignored, with the raw flag histogram.
- Render, ready and publish p50/p95 per job kind, and the time spent in `SetOverlayTexture`.
- Prediction horizon, and the residual: the predicted eye against the unpredicted pose sampled at the predicted display time.
- Turn-period p50/p99.
- Headset: `TRACKING_SYSTEM_NAME`, `MODEL_NUMBER`, refresh, photons property, both eyes' tangents and wide sizes.
- The distribution of `compositor_render_start_ms`, to estimate the latch phase.
- VRChat frame intervals with and without the capture shown, from `FrameSampler`.
- A hidden switch back to the old policy, so live A/B comparison is possible.

## 9. Required release caveat

> Pacing and prediction were validated offline only. SteamVR's overlay latch timing, the impact on VRChat's frames, and behaviour per headset driver (Virtual Desktop or Link) are unverified until a live session with diagnostics. Relaxing VRChat protection while the capture is opaque (VRChat is then hidden) is a product decision deferred to Enzo.
