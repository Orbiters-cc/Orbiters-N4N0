# VR1111 behaviour implementation

Status: implementation and local validation complete; headset acceptance remains open. Implementation head: `6d73b7f7da98b39045b601841ec20e8efd70f11a` on `vr1111/behaviour` (from `cf908c6`). No release, push, version bump, installed-app launch, SteamVR/VRChat interaction or GPU benchmark was performed.

## F21 fast walk

Changed `overlay/walk.rs`, `overlay/mod.rs`, `controller/n4n0.rs`, `state.rs`; timestamp-only fixture updates in `ipc.rs`, `service.rs`, `link_smoke.rs`.

Verified today's `walk-boost.log` lines 86-95: repeated 0.81/0.82 s fast and roughly 0.506 s normal. The old gate subtracts the previous frame's injected speed from a potentially delayed scalar OSC magnitude. This was the direct cause of rejection under the replayed native-speed values; the log itself contains no raw speed that could prove the injected component. Incoming `VelocityMagnitude` is supplied by VRChat; the controller previously quarter-rounded it and supplied default zero even when only a station field had arrived. It never measures or constructs native-plus-injected speed.

The replacement does not subtract speed or periodically retry on a timer. Positive raw telemetry keeps a push walking; explicit near-zero speed stops after the existing 300 ms debounce. Station/seated, menu/calibration, away, scene/input/tracking and stick release gates remain. Missing samples preserve the last established carrying state; an established stop is not restarted merely by missing telemetry. A station-only update does not masquerade as an observed zero speed. Source receive time and unrounded speed are published separately. Age is diagnostic, not a lease: change-only OSC can leave valid values old. Transitions and one-second pushed-stick samples log stick, raw speed, receive age, station flag, gate result, input/held state and previous applied boost. Walk start/stop distances now use the driver's carry when active instead of the unrelated chaperone offset.

Clock-controlled tests cover 30 s at 72/90 Hz and several strengths, missing/delayed/unchanged samples, zero-speed stop, station, release and re-push. A second regression drives the complete fake driver continuously for 30 s at each rate and three stick positions. Existing tests retain menu/away, tracking, recenter, other-app drag/turn and UI-stall coverage.

Evidence limit / research correction: the old code comment and any interpretation of the reports that asserts a known room-motion contribution are unsupported. The official parameter documentation defines total velocity magnitude, not an additive decomposition: https://creators.vrchat.com/avatars/animator-parameters/ . Today's logs cannot establish that contribution. A world freeze still emitting positive speed cannot be distinguished from walking using this scalar alone. Zero-speed freeze is covered; positive-speed freeze and collision/comfort/delivery require live VR acceptance. No native observable was invented and no timer was lengthened to mask the cycle.

## F20 smooth tracking sliders

Changed `overlay/orbit/mod.rs`, `backend.rs`, `ipc.rs`, `service.rs`, `state.rs`; regression flow in `overlay/orbit/flows.rs`, screenshot scenarios in `screenshot.rs` and `screenshot/orbit.rs`.

Device thumbs now read configured top-level tracking settings rather than delayed N4N0/driver status. Driver availability and its note remain separate. Release uses the latest desired device array, including preceding pending edits. Dedicated numbered smoothing patches survive unrelated old snapshots. The service acknowledges a patch after applying/clamping/refusing it and publishes a smoothing revision. A pending value clears only after both that acknowledgement and a state at its revision arrive. Thus the value settles once on the authoritative result, including refusal, without a release-to-old-value interval. Menu and wheel patches use the same mechanism. No settings write is sent from painting and no timeout guesses when an edit succeeded.

Regression coverage includes stale snapshots, acknowledgement before its state, clamp/refusal, two device sliders in quick succession, Quick Menu and wheel values, and configured thumb values while the derived driver view remains old. Five screenshot states: held, released, stale-driver, two-sliders, clamped. Visual review also found long translated row/value labels reaching a track. Row names now fit their column and the value column reserves 132 units, with pointer-to-value mapping using the same revised bar endpoints. No new translated user text was introduced.

## F4 Boost FPS honesty

Changed `orbiters-booster/src/engine.rs`, `classify.rs`, `tests/fixtures/2026-11-11.jsonl`, desktop `controller/booster.rs`.

Off cancels the running trial and clears all retained engine changes on that tick. The service also refuses a stale enabled-level report while the actual setting is Off. An explicit Off test remains allowed and is marked manual. StopTest cancels it; PutBack issues StopTest even when the setting was already Off. Failed process restoration remains pending for retry instead of being forgotten. A SteamVR refusal interrupts the lifecycle rather than leaving it marked running.

Processor diagnosis now requires CPU-late reason flags; the wait-inclusive app CPU estimate alone cannot assert a processor bottleneck. Observed half-rate pacing is a distinct report field. GPU timing remains an indicator of GPU pressure; no transport/headset root cause is asserted. No new automatic optimization was added and no gain is synthesized.

Today's fixture retains measured block summaries and original level/trial/lowered fields from roughly 11:38:24-11:40:40 UTC, stripping process names and world identity. The regression reconstructs constant frames from those summaries and feeds Engine. It proves policy against the supplied measured summaries, not the original timing distribution. It never reports a processor limit from those wait-inclusive estimates, never produces a measured gain from those unpaired blocks, and applies no automatic changes at Off. Separate controlled trials exercise Off cancellation and explicit Off test stop.

Correction to map-features.md: support for an explicit test at Off does not establish that today's continued trial was explicitly requested at Off. The report's broader caution was appropriate; the level transition cancellation defect is directly reproduced by the engine test.

## Merge and interface contract

This stream adds IPC messages and snapshot fields; service and overlay must ship together. No compatibility fallback was added. Shared files touched outside the narrow implementation area are listed above; retain their small hunks when merging shell/pages/paint/camera work. `vr1111/pages` owns the new Boost FPS page wording and presentation.

## F4 interface contract for vr1111/pages

`Snapshot.booster.overlay.report` now includes:
- `level`: the setting used to produce this report. Consumers must not apply an older enabled-level report after Off was selected.
- `trial_status`: persistent `idle`, `automatic`, `manual`, `cancelled`, `completed`, `interrupted`. Unlike `line`, this does not expire after 30 seconds.
- `testing`: running kind / side / seconds_left, plus `manual: bool`. An explicit test at Off is a visible manual test; Off itself is not an active boost.
- `now.half_rate_pacing`: observed real FPS within 5% of half refresh with >40% filled frames. This is a pacing observation, not a processor diagnosis.
- `comparisons`: existing persistent result per kind, with verdict, measured before/after FPS/stalls and kept. Show NoGain as no gain. `Kept` verdict with `kept=false` means measured improvement but restored because the level did not permit keeping it. No comparison means no measured gain, not zero or an estimate.
- `wants`: engine intent. For actual application use existing `Snapshot.booster.lowered` (program count), overlay `sharpness` (worker readback), and restoration information. Intent is not proof of OS success.

Stop action: `Command::Overlay(Request::StopTest)`. Existing `Command::PutBack` now also sends StopTest, including if already Off. UI should offer a Stop for every running manual trial, not just a level switch. Existing Put everything back is valid in this stream pending the pages redesign.

The Boost FPS page wording and layout are owned by the pages stream. Pages must translate newly displayed lifecycle wording in all five languages and add screenshots for these states. The engine no longer identifies a processor limit from the wait-inclusive app CPU estimate. Reason flags still support processor/graphics classification; GPU timing remains a GPU-pressure signal. The replay fixture contains measured summaries, not raw frames: its synthesized constant frames test policy only.


## Validation actually run

All Cargo commands used `C:/Users/Enzo/.orbiters-build/sota/bench/deskcargo.sh` with this worktree, the existing shared target and `CARGO_INCREMENTAL=0`. No target directory was created. Sources stayed frozen for each queued/running batch. The queue overtook the first final batch repeatedly; that batch was cancelled before compilation to include final review fixes. An earlier compile exposed two timestamp-less AvatarMotion test fixtures, corrected before the successful run.

- `cargo test -p orbiters-booster -p orbiters-desktop -p orbiters-i18n --no-fail-fast -- --test-threads=1`: exit 0. Booster 123 unit + 2 integration passed, 4 ignored. Desktop 811 passed, 25 ignored. i18n 10 unit + 7 coverage passed. No failures. Log: `C:/Users/Enzo/.orbiters-build/vr-feedback-1111/test-behaviour-final.log`.
- After the layout-only follow-up: `cargo test -p orbiters-desktop tracking_sheet -- --test-threads=1`: exit 0, 4 passed. `cargo test -p orbiters-i18n`: exit 0, 17 passed. Logs `test-behaviour-tracking-layout.log` and `test-behaviour-i18n-layout.log` in the same evidence folder.
- `cargo build -p orbiters-desktop`: exit 0 before initial renders and again after layout review. Final log `build-behaviour-layout.log`. Existing unrelated compiler warnings remain; no claim of a clean clippy run.
- QA binary copied immediately out of the shared target; renders ran with isolated harness settings/cache folders, as printed in their logs. For each of en/fr/ja/es/pt-BR: `--screenshot <folder> --only orbit-behaviour --language <language>` succeeded. Five tracking states per language. Also rendered the existing `--only settings-booster` scenario in all five languages. Every initial and final picture was opened and inspected.
- Initial 30 renders: `C:/Users/Enzo/.orbiters-build/vr-feedback-1111/shots-behaviour/<language>/`. Final 25 tracking renders: `.../shots-behaviour/final/<language>/`. The Spanish overlap found in the initial renders is fixed in final renders. Existing header-title elision and side tiles remain shell-stream territory. The stock settings-booster screenshot uses its pre-existing synthetic 36-to-41 FPS comparison fixture; it is not a measurement of Enzo's session or a claimed gain.
- `git diff --check`: clean. Regression checks were run after implementation; the old checkout was not separately rebuilt with the new tests. The new gate, configured-vs-derived UI flow, and cancellation assertions directly exercise the old failing paths.

## What is not verified / acceptance and blockers

No headset test, live VRChat/SteamVR session, real process-priority mutation, live sharpness trial, or macOS build was run. No measured FPS improvement is claimed. Engine/service fake tests establish cancellation, policy and restoration intent; actual runtime ownership/restoration remains a release acceptance check.

For Enzo's VR acceptance after the orchestrator's combined release: hold forward 30 s at low/mid/full stick at both refresh rates; enter a station, open a menu, toggle away, try a world freeze, release and re-push. Review the new raw-speed/age/gate log if any stop occurs. In particular, freeze while raw speed remains positive is unresolved by this telemetry and must not be reported as proved fixed. Validate slider release and quick consecutive edits against the live driver; verify an explicit Off FPS test is visibly distinct and stoppable through the merged pages UI, and that Off/PutBack actually restores journalled changes.

The pages stream must consume the fields above and render the manual-test stop and persistent verdict/no-gain states. Merge shared-file hunks carefully, then run the integrated release/headset checks. These are acceptance dependencies, not compile/test blockers in this branch. The QA executable is an isolated verification artifact, not a build for Enzo to try. No push, tag, bump, release or merge was performed.

Commits before this report: `7e9065d8` (F4), `82cf7e6c` (F21/F20), `472b302a` (review and regressions), `6d73b7f7` (translated slider spacing). The report is committed separately; its final branch head is recorded in the external done file after that commit.

## Gate correction

This section supersedes the Review fixes motion-interlock model and its positive-residual/latch acceptance claims. Scope is F21 only, on vr1111/behaviour from 7f97aad5c5b1c66ad3dd1bf3ade159670ea46aa2, in its existing t3-e5f2db86-40156b52483716b7 worktree. The main checkout was not edited.

Verified the finding in overlay/walk.rs: raw speed <= previous injection + 0.15 started the 300 ms debounce, then stopped until release. The owner's 13:04:30-13:04:41 log records repeated 0.81/0.82 s motion-gate stops. That comparison rejects native-only speed below injection and recreates the original error; a release latch makes it persist for the rest of the push. The earlier research correctly identified the subtraction problem; the first review's conservative replacement was wrong for the requested continuous walking.

Gate now treats VelocityMagnitude as VRChat's own locomotion permission. Its API no longer accepts injected speed. Start requires finite speed >= 0.4 m/s; after starting, speeds >= 0.15 retain permission, while lower speed, missing Motion, or nonfinite speed stops after 300 ms. It resumes at >= 0.4 on the same push. Old timestamps do not invalidate a present value. A stationary initial push waits for native motion rather than getting a timed 500 ms allowance; no allowance is necessary because normal joystick locomotion starts independently of pose injection. No retry timer or release latch remains. A station/seated prohibition still survives missing telemetry, release and re-push until a present report says non-station. All existing scene/menu/away/input/tracking and driver acceleration rules remain. The diagnostic no longer tells the user to release to retry.

No solid evidence of an additive injected component on either path was found in the inspected source or feedback analysis. Both paths consume the same VRChat OSC scalar; neither constructs native-plus-injected telemetry. The same native gate is used for the play-space fallback, without a speculative compatibility comparison. A frozen world continuing to report a positive scalar is still indistinguishable from walking with these inputs.

Tests changed and why:
- continuous_push_never_interrupts_driver_at_72_or_90_hz now supplies native-only 4, 2 and 1 m/s while full injection reaches 8 m/s, plus partial-stick 2/1 m/s cases. Every frame after the initial zero-time frame must move throughout 30 s; driver speed and Driver path are asserted. Constant timestamp 1 is intentional.
- carries_only_while_vrchat_walks uses native-only 1/2/4 instead of 12*strength, retains transient missing samples and now requires recovery without release.
- review_positive_residual_stop_latches_until_release is replaced by native_speed_resumes_without_release_and_threshold_rounding_does_not_flap. The old test asserted the wrong residual model and wrong release latch. Replacement tests start threshold, 0.25/0.5 rounding, the 300 ms zero stop, 30 s of remaining stopped, and recovery while still pushing.
- review_driver_station_missing_and_positive_frozen_motion_never_cycle is replaced by review_driver_station_missing_and_zero_motion_stop_and_resume. Controlled clocks at 72/90 Hz verify gate rejection by 300 ms plus one frame, unchanged offset after deceleration during sustained zero/station, station persistence across release/re-push and missing telemetry, and recovery on a positive native/non-station report without release.
- driver_quarter_metre_speed_rounding_does_not_flap verifies exactly one permission transition over 30 s of 0.25/0.5 m/s samples.
- The common Moment fixture now uses native 4 rather than artificial 100 m/s. The play-space long-walk noise fixture no longer adds a synthetic 8 m/s to native speed. Existing distance, continuity and jump assertions are preserved.

Validation: one queued batch, gate-batch.sh, using the existing shared deskcargo target with CARGO_INCREMENTAL=0. Sources remained frozen throughout queue waits and execution. No new target directory was created.

1. deskcargo.sh <behaviour-worktree> test -p orbiters-desktop overlay::walk -- --test-threads=1: 32 passed, 0 failed, 0 ignored; 0.83 s test runtime; captured Cargo exit 0. Log: C:/Users/Enzo/.orbiters-build/vr-feedback-1111/gate-walk.log; exit: gate-walk.exit.
2. deskcargo.sh <behaviour-worktree> test -p orbiters-desktop -p orbiters-i18n --no-fail-fast -- --test-threads=1: desktop 816 passed, 0 failed, 25 ignored, 308.10 s; i18n 10 unit + 7 coverage passed, 0 failed; doc tests passed. The desktop child-process test also reports its one selected test passed. Captured Cargo exit 0; batch exit 0. Log: C:/Users/Enzo/.orbiters-build/vr-feedback-1111/gate-desktop.log; exit: gate-desktop.exit. Existing unrelated warnings remain; clippy was not run.
3. git diff --check passed. No UI layout or user-facing wording changed, so no screenshot rendering was needed.

The new driver continuity assertions directly reject the previous comparison/latch implementation; an old-production Cargo rebuild was not performed this round. The full suite naturally executes the walk module again; the dedicated walk command and the full-suite command were each invoked once.

Headset acceptance remains: continuous low/mid/full-stick walking for 30 s at 72/90 Hz; wall/menu/world-zero stop and same-push recovery; station entry/exit and telemetry interruption; acceleration comfort and OVRAS interactions. No live app, SteamVR or VRChat launch/settings change, GPU benchmark, macOS build, push, release, bump, tag or merge was performed.
