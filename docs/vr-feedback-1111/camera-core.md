# Camera core handoff - VR feedback 2026-11-11

Branch: `vr1111/camera-core`.
Base: `cf908c67a3d05239e18bf25a0d2251b3fd767769` (0.3.5-dev.37).
Implementation head: `4425641c2132200e01e47a83a62ef04ccf7f86b6`.
Final validation: **60/60 camera/controller tests passed on 4425641c**, including the exact tilted-camera test, full clearance hull and held-menu takeover. OpenVR and i18n passed. Empty/cancelled logs are unexecuted checks, never passes. The final test evidence is `camera-core-camera-delivery.log` (non-empty, exit 0).

No push, version bump, tag, release, app launch, settings change, real avatar selection or GPU benchmark was performed.

## Limits that must remain visible in the merge/release report

- Immediate tools have a **marked provisional location**. Exact VRChat camera spawn placement and calibrated reopening are **not delivered**. No trustworthy world-to-Standing relation or optical-camera-to-panel extrinsic was established from the available inputs. A coincident pair alone contains both unknown transforms; locomotion/recentering also invalidates a cached world relation. No fabricated calibration is saved.
- A visual fit now recovers a perspective plane and validates its rigid reprojection. Frame-matched right/left observations recover metric scale using both eyes' true projections and eye transforms. Native stereo acquisition, binocular alignment and real tilted-panel recall need headset validation. Mono-only fitting uses the last measured width, or `camera_panel_m` (default 0.30 m), and remains provisional. F18 must not be called headset-proven fixed from synthetic tests.
- Recognition retains the X/chevron candidate seed and refines independent spread landmarks. It is not a complete independent outline-acquisition replacement. There is no measured improvement over today's 99/1334 live recall. The provisional fallback prevents a blank camera-tool UI while recognition is uncertain.
- `ORBITERS_N4N0_DEMO_AVATAR` is the single canonical compile-time configuration. It is unset. The orchestrator needs Enzo's real public demo avatar ID before a switch can be offered.
- Camera-ui owns the artwork, translated provisional/kit explanations, removal of outside-camera Record buttons, and wiring of its camera Record/QR/Demo actions. Camera-web owns the embedded page above the camera and explicit browser opening. This branch supplies behavior/state, not those views.

## Per item

### F9/F19 - opening, takeover, immediate placement, reader lifetime

Files: `controller.rs`, `overlay/orbit/vr.rs`, `camera_frames.rs`, `state.rs`.

Received `/usercamera/Mode` publishes straight from `on_osc`, after releasing `osc_values`; it no longer waits for the keeper. A connection bootstrap and keeper seed an unknown state from the camera-only OSCQuery node. A query seed controls visibility but does not itself authorize recording.

Both None->true and false->true close the wheel and Quick Menu companion and `close_sheet`. Takeover cancels pending menu presses and deferred Quick Menu toggles, so a still-held press or its release cannot reopen them over the camera. The camera takes over menu-owned unheld 3D previews. A held view receives independent ownership and survives; views opened by the avatar's independent menu are not hidden. Camera tools get a stable provisional plane on the first valid head pose, without waiting for two detections. The provisional flag remains set while a fit awaits corroboration, adoption, settlement or recent metric stereo evidence. Adopted target corrections settle instead of teleporting. Grip following requires an adopted pose within 1 cm, 2 degrees and 1% scale of its target plus recent stereo; an already-held tracked panel retains its grip. A grip near the guessed fallback cannot bind that guess to a controller.

Normal reader closing only sets a stop flag. `OrbitVr` retains unfinished readers, reaps finished handles, and joins any remainder at teardown before `Teardown`'s borrowed SteamVR client can die. Reader-end logging runs on the worker. No join, thread spawn, health-lock read or file append occurs on the normal close path. Tests cover close latency and final joining with a held fake reader.

### F18 - perspective tracking and submitted hulls

Files: `camera_track.rs`, camera parts of `vr.rs`, `camera_frames.rs`, `orbiters-openvr/src/compositor.rs`.

Candidates match at least five unique fixed landmarks, including an anchor on the right, rather than deriving a live plane from the X/chevron alone. An eight-parameter least-squares homography rejects pixel RMS above 2.5. Decomposition uses the real left-eye projection/eye transform, rejects inconsistent rigid axes and depth outside 0.15..4 m, then reprojects the recovered **rigid plane** and rejects RMS above 2.5 px or maximum point error above 6 px. A perfectly fitted sheared texture therefore cannot qualify as a camera plane. Strong roll needs projective evidence. Candidate ranking retains the original glyph/support score; a fit gets no ranking bonus.

After acquisition, the reader reads both eyes before recognition and pairs them only when GetFrameTiming reports the same compositor frame index before and after both reads. Stereo triangulates the four plane corners using both real projections and eye-to-head transforms, rejects divergent/behind-eye rays, inconsistent aspect and depth, and reprojects all measured landmarks into both eyes with the same 2.5 px RMS / 6 px maximum bounds. It measures physical width without assuming 0.30 m. Width is cached across openings; the metric-evidence timestamp is reset on opening/closing. `camera_scale_measured` requires stereo accepted within two seconds. A missing or rejected right view falls back to provisional mono. The frame-index check avoids knowingly pairing different compositor turns; it does not establish the exact render-pose age of the mirror texture.

Side selection rejects a column crossing the eye projection plane. A separate live acceptance guard projects a conservative column hull, including the full uncertainty clearance, from the recovered rigid pose and requires it to lie outside the observed panel edge; stereo checks both eyes. It tries the other side, then rejects the fit and returns to detached provisional tools. A low landmark RMS alone cannot authorize an overlapping column. The guard also rejects a column whose added clearance crosses the eye plane. Loss of detection returns to a persistent fallback instead of hiding all tools after the tracking timeout.

The lateral gap adds max(12 mm, (2.5 + measured RMS) pixels converted through the observed scale) to the existing 10-unit gap. This clearance moves **only the column** through `camera_column_out`; the fitted frame stays centred on the panel, so QR corners and the web page do not drift. Preserve camera-ui's richer helper, its CameraRecord/CameraAct cases, and its `(clearance + 10 - CAMERA_GAP).max(0)` adjustment for its 18-unit baseline gap. `Plane::camera_quad` submits camera tools in the fitted plane with zero shell depth offset and zero radial spring displacement. Other shell quads keep their existing behavior.

The synthetic perspective sweep covers pitch, yaw, roll, distance, panel width and both sides. It checks separating edges and the actual submitted CameraPeople/CameraWorld/CameraQr quad corners. Non-projectable sides are rejected. A separate sheared-texture regression checks physical-plane rejection. Stereo ground truth covers canted/asymmetric eyes, varied physical widths and a mismatched pair. Existing positive fixtures and negative game/menu frames retain their assertions; an additional real-fixture assertion requires spread landmarks.

Diagnostics log measured landmark pairs, homography/RMS, both read instants and compositor pair index, acceptance instant, common-clock acceptance ns, depth, stereo RMS/width, side, gap, pose, Mode receive/ignored-echo timestamps, and first camera-zone show submission. These are submission/read timestamps; actual compositor frame age and presentation latency remain unmeasured.

### F13/F14/F15 - stop, camera evidence and start origin

Files: `controller.rs`, `controller/burst.rs` (camera query visibility), `ipc.rs`, `backend.rs`, `service.rs`, `state.rs`, `tray.rs`.

Every live Spout source passes the shared camera preflight before state mutation, kit/camera changes or worker Start. TestPattern and raw imports are exempt. Preflight requires a fresh camera-only query, open state, and either received Mode history or a physical panel fit accepted within two seconds. Query seeds alone are insufficient. App Mode writes carry a bounded two-second echo marker; their matching echoes cannot create authoritative camera-open evidence. Received close and OSC reconnection invalidate panel evidence. Cleanup does not restore a nonzero Mode after a real camera-close observation. Resume checks the live camera too.

The guard covers long recordings, short pictures, scan, avatar record toggle, queued/restarted capture and IPC. Starts recheck camera/cancellation immediately before Worker Start. A stop generation cancels preflight/queued starts; start serialization prevents concurrent duplicate starts. IPC keeps reading while long preflight runs, so Stop is not trapped behind it. Actual Spout frame readiness is not separately awaited.

`StopCapture` is stop-only and has no camera, origin, kit or OSC precondition. It cancels pending starts and retains the existing worker-stop/save/cleanup path. The tray now offers Stop only.

Public long-recording requests carry `RecordingOrigin`; only `CameraOverlay` is accepted. Generic backend UI start sends `OtherUi` and is refused. The internal boolean start helper is private to the controller and its children; avatar and short-capture exemptions remain internal and still pass F14. Camera-ui must use `start_camera_recording`, not generic `start_capture`.

`capture.live` identifies a live Spout operation. `Snapshot::live_recording_running()` means live Starting/Recording/Paused, including short operations; it excludes import/test pattern and Stopping. Use capture.state to show Starting, Paused and Saving accurately.

### F16 - kit knowledge and explicit demo switch

Files: `controller.rs`, `controller/camera_tools.rs`, `state.rs`, `vrchat_api.rs`, `vrchat_source.rs`, IPC/backend/service and controller translations.

`worn_kit` retains Some(true)/Some(false)/None. `capture_kit_reason()` yields ready / avatar_has_no_capture_kit / avatar_capture_kit_unknown. Parameter presence is capability evidence, not proof of a correctly drawing shader. `can_switch_demo_avatar()` requires configured ID, running VRChat, allowed signed-in session matching the log's playing ID, no capture/save and no switch already busy. The controller enforces that same predicate before beginning selection; it is not only a UI visibility check. Camera-ui also conditions the offer on its kit state.

Kit queries carry an avatar generation. Capability is invalidated before a new avatar ID is published; old/in-flight lookups cannot overwrite the new avatar's unknown state. A result for a new expected avatar cannot publish before its avatar payload arrives. This protects disabled tiles and demo completion from stale old-avatar kit evidence.

The explicit switch uses the existing private signed-in client: authenticated PUT `/avatars/{avatarId}/select`, no body, no server fallback. `Source` requires known playing identity and a fresh matching authenticated user. Busy includes waiting for a newly observed target avatar ID and refreshed positive kit state; HTTP success alone is not Done. Account change and timeout produce Error. It never switches automatically or starts recording.

### F17 - camera-scoped QR consumer

Files: `controller/camera_tools.rs`, controller/state/IPC/backend/service.

The camera owns a separate QR scan handle and generation. It publishes Off / Looking / NoStream / Found { url } / Error. URLs use the existing N4N0 `web_link` validator. Found remains available when the stream stops. Closing the camera drops this consumer and invalidates old callbacks. It neither navigates the large QR page nor invokes Host::watch/open_url. Starting it while the camera is not known open publishes NoStream. Camera-web opens externally only on an explicit user action.

## Exact state and command contract

- `Backend::start_camera_recording()` -> `StartCapture { origin: CameraOverlay }`.
- `Backend::stop_capture()` -> `StopCapture`, including pending/preflight cancellation.
- `Backend::camera_qr(bool)` -> `CameraQr { active }`.
- `Backend::switch_demo_avatar()` -> `SwitchDemoAvatar`.
- `Snapshot.camera_tools.qr`, `.demo`, `.demo_avatar`, `.qr_generation`.
- `Snapshot.worn_kit`, `capture_kit_reason()`, `can_switch_demo_avatar()`.
- `Snapshot.capture.live`, `.state`, `.photo_3d`, `.picture`, `live_recording_running()`.
- `Snapshot.orbit.camera_provisional`, `.camera_scale_measured`, `.camera_panel_seen_ns`, `.camera_note`. These publish live independently of SteamVR's one-time input-override setup. `camera_panel_seen_ns` is fit-acceptance time from the common orbiters_core clock, cleared on close/reconnection. `camera_scale_measured` means stereo accepted within two seconds; it is cleared across openings. Mark mono-only or still-settling placement as provisional.

```json
{"type":"start_capture","origin":"camera_overlay"}
{"type":"stop_capture"}
{"type":"camera_qr","active":true}
{"type":"camera_qr","active":false}
{"type":"switch_demo_avatar"}
```

Qr/Demo use serde external enum tags: `"Looking"`, `"NoStream"`, `{"Found":{"url":"https://example.com/"}}`, `"Busy"`, `"Done"`, `{"Error":"message"}`. Prefer Backend methods and typed Snapshot fields in Rust.

## Placement contract for camera-ui / camera-web

SteamVR Standing, metres, +Y up, -Z forward. Panel local coordinates are 410 x 220 units about its centre, UI Y down; `unit = measured_width_m / 410` after stereo, otherwise last measured width / 410 or `camera_panel_m / 410`. This branch's base tiles are x=-301..-215 with y=-110..110; camera-ui replaces that layout. Right side mirrors x. The fitted frame remains centred; `camera_column_out` adds uncertainty clearance to column placements only. `Plane::camera_quad` uses local Z=0 and suppresses radial placement springs; texture/alpha animation can stay in the UI stream.

Provisional frame: `before_eyes(head_at_open, 0.65 m, 0.22 m below)`, frozen in Standing. Initial side is Left, placing the detached tool column left of centre (the supplied camera examples are right of centre). It is not a known VRChat panel pose or a proven spawn offset. Mark it as locating/provisional using `camera_provisional`; this remains true through settlement. Visual tracking uses Follow with an 8/s settlement rate.

For a browser above the camera, compose with the centred camera plane frame, move local +Y by `110 * unit + gap_m + browser_height_m / 2`, and retain local Z=0 (camera-ui supplies the final hinged top-plane transform). Do not use the old head-facing/left-side takeover transform or apply the column's lateral clearance to the browser/QR corners.

## Validation and evidence

All Cargo commands use `C:/Users/Enzo/.orbiters-build/sota/bench/deskcargo.sh` with this worktree, the existing shared target, CARGO_INCREMENTAL=0 and one test thread. No new target directory.

Completed:
- `cargo test -p orbiters-desktop --bin orbiters-desktop -- camera controller::tests --test-threads=1`: **60 passed**, 0 failed on 4425641c, 146.37 s in the test binary, exit 0. This is the final complete camera/controller run, including the exact tilted-plane test, full-clearance/eye-plane regression and held-menu press/release regression. Log: `camera-core-camera-delivery.log`.
- `cargo test -p orbiters-desktop --bin orbiters-desktop -- camera controller::tests --test-threads=1`: **59 passed**, 0 failed on f8ca38f5, 143.92 s in the test binary. Includes the exact tilted-plane test reported by camera-ui (retained hull assertion plus stricter recovered-axis/position assertions), the entire camera set, real-fixture spread landmarks, asymmetric/canted stereo metric scale, mismatched-pair rejection, low-residual/overlapping-column rejection, controller eligibility, stop/evidence/kit races and reader lifecycle. Log: `camera-core-camera-full.log`.
- `cargo test -p orbiters-openvr -p orbiters-i18n -- --test-threads=1`: **OpenVR 10 passed, 1 live-runtime test ignored; i18n 10 unit + 7 coverage passed**, 0 failed on f8ca38f5. Includes the mocked GetFrameTiming ABI call. Log: `camera-core-platform-i18n.log`.
- `cargo test -p orbiters-desktop -- camera controller::tests --test-threads=1`: **51 passed**, 0 failed on 67e42dd6. Includes image fixtures, negative frames, synthetic perspective, rigid-fit rejection, camera Mode/app-echo preflight, stop cancellation, cleanup, reader close/final join, and controller regression modules. Log: `camera-core-accept-tests.log`.
- `cargo test -p orbiters-core vrchat_ -- --test-threads=1`: **67 passed**, 0 failed, 2 unit tests ignored; 2 live integration tests also ignored. Mocked authenticated selection, wrong/unknown playing-account refusal and existing session/cache/revocation tests. Log: `camera-core-final-api.log`.
- `cargo test -p orbiters-i18n`: **10 unit + 7 coverage passed**, 0 failed. New timeout wording is present in fr/ja/es/pt-BR plus the English source; the new source file belongs to the controller catalog. Log: `camera-core-accept-i18n.log`. No user-visible wording changed afterward.

OpenVR/i18n/API source and user wording are unchanged after their respective passing runs. The preceding empty `camera-core-camera-final.log` was cancelled before lock acquisition to add the held-press correction; it is not a result. No implementation source changed after the final 60-test run; the last commit only records documentation and the annotated cases.

Earlier failures were kept visible and corrected: camera_values visibility; old start-failure fixture without camera evidence (now a fake camera-only query, preserving cleanup assertions); wrong-account mock missing its permitted identity GET; projective ranking bonus selecting two wrong X glyphs (bonus removed, assertions retained). The preceding submitted-quad run passed 35 tests and failed one test that projected absolute Standing coordinates as eye coordinates: it now subtracts the synthetic eye's 1.5 m origin before projection, retaining the hull assertion. Several queued reruns were cancelled before lock acquisition to apply corrections. Their Windows shell zero exit codes are **not** counted as executed tests. One final edit occurred while an older test was queued; that empty-log job was cancelled and its result discarded. Only the fresh final run counts for the last additions.

Orchestrator follow-up: camera-ui's merge at c57096c7 reproduced that same submitted-quad failure at pitch -60, yaw -55, roll -45, distance 0.3 m, width 0.2 m, right side. A separate CPU calculation using the exact ground-truth pose and camera-ui's inner corner (223, -50) gave the unchanged edge-product assertion **-0.00062244** (outside, correct eye-relative coordinates), versus **+0.08836396** (incorrect projection of absolute Standing coordinates). Thus the reported failure can be produced with a perfect pose by that coordinate-space error. This calculation is diagnostic, not a replacement for the Rust run. The follow-up retains the hull assertion, strengthens pose assertions, adds live hull rejection, and preserves column-only clearance. The old empty stereo log was cancelled before editing and is not a result.

All six REQUEST screenshots were opened. `camera-core-cases.html` and `.json` annotate IMG3/4/5 with approximate manual panel/tile polygons. They are original feedback, not new implementation renders or calibrated raw-eye ground truth. IMG3/4 show overlap/different perspective; IMG5 shows missing tools but cannot prove elapsed delay. The originals remain in the user attachment folder.

Today's installed logs independently confirm the 1334 looks / 99 finds session and 166/228 ms closing-turn stalls. These support the research leads; they do not identify the blocked stage in an individual screenshot. The reports' source pointers were accurate for the base. Their coincident-pose calibration suggestion is insufficient by itself to identify both unknown transforms. The existing T-pose calibration does not supply VRChat-world alignment. Official camera docs publish Pose/Flying, not an exact numeric head/hand spawn transform.

Primary sources checked November 11:
- [Community selectAvatar specification](https://raw.githubusercontent.com/vrchatapi/specification/master/openapi/components/paths/avatars.yaml)
- [VRChat camera OSC endpoints](https://docs.vrchat.com/docs/vrchat-202533)
- [Official camera guide](https://wiki.vrchat.com/wiki/Camera)
- [Official OSC reference](https://wiki.vrchat.com/wiki/OSC)
- [Valve OpenVR compositor ABI](https://raw.githubusercontent.com/ValveSoftware/openvr/master/headers/openvr_capi.h)

## Merge and remaining verification

Camera changes cross shell territory only in `overlay/orbit/vr.rs`, plus the desktop OrbitStatus constructor for the added fields. Preserve camera takeover, dynamic camera status, retired-reader ownership, zero-offset camera quad path and held-view ownership when merging shell/camera-ui edits. `vrchat_api.rs` and `vrchat_source.rs` overlap instances-api: retain the narrow select_avatar additions and mocked tests. The OpenVR change is a small GetFrameTiming wrapper and mock ABI test used for frame-pair rejection. No gallery/page design, paint pipeline, walk engine or TouchWorld work was changed.

Headset checks remain: real Mode-to-first-visible latency, default/provisional comfort, fit recall and false positives in varied worlds, native frame-matched stereo acquisition, both eyes at strong tilt and scale, real grab/release/Fly behavior, close/reopen/reconnection, actual Spout scanning, and explicit demo selection after a real ID is configured. macOS compilation/runtime was not verified on this Windows host. Exact calibrated reopening and independent outline acquisition remain undelivered. No release/build-to-try is claimed; the orchestrator merges streams and publishes one development release.
