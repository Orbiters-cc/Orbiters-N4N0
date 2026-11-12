# Developer VR sandbox

The development sandbox runs real SteamVR and VRChat with an emulated headset
and two `oculus_touch` controllers. It is separate from the tracking smoothing
driver and is not shipped in the installer. Its driver refuses activation without
a shared-memory session owned by a live controller process.

The feedback directory's `sandbox-ready.md`
is the readiness handoff; this guide alone is not a completion claim.

## Build and start

From this worktree, use Windows Python with `psutil` and Visual Studio C++ tools:

```powershell
& tools/vr-sandbox/build.ps1
& tools/vr-sandbox/build-motion.ps1
python tools/vr-sandbox/sandbox.py start
```

The controller stays running. In another terminal, or an agent tool call, use
`python tools/vr-sandbox/sandbox.py request '<JSON>'`. PowerShell callers can use
Python's `sandbox.remote({...})` directly to avoid shell JSON quoting.

Build Rust through the shared `deskcargo.sh` queue, with its existing target.
Stop and restore your SteamVR/VRChat session before compiling. Keep live capture
batches short, then restore and release the lock so other streams get a turn.
Do not delete another stream's build artifacts to make space.
The sandbox bridge adapter exists only in debug builds and only when
`ORBITERS_VR_SANDBOX_ROOT` points to this worktree's `tools/vr-sandbox` directory.
The normal bridge's authentication and feature gate remain in effect.

The session refuses existing VR processes and takes
`C:/Users/Enzo/.orbiters-build/vr-sandbox/steamvr.lock` exclusively before changing
anything. Do not remove another stream's lock. No headset or VRChat process that
the sandbox did not launch may be stopped.

## MCP control

The existing `orbiters-vrchat` MCP server exposes `vr_sandbox` in the enabled
development process. Its actions are:

| Action | Arguments and result |
| --- | --- |
| `start` | Starts the standalone session controller and SteamVR. |
| `status` | Recorded processes and commanded head/hand states. |
| `vrchat` | `world_id`; creates a strict private instance using the bridge's signed-in account and starts VRChat through its EAC launcher, without `--no-vr`. |
| `pose` | `device`: head/left/right, `position`: Standing metres, `rotation`: quaternion **w,x,y,z**, optional `duration_s` 0–30. A duration interpolates smoothly at 144 updates/second. |
| `input` | `device`: left/right, `input`, `value`; optional `duration_s` releases it after a bounded press. Otherwise release explicitly with zero. |
| `capture` | `output_prefix`; saves `-left.png` and `-right.png` from SteamVR's composited eye textures, including overlays. |
| `motion_capture` | `device`, optional target `position`/`rotation`, `duration_s`, `frames` 3-80, `output_prefix`; timed native eye PNG pairs plus JSONL with QPC readback intervals, scene frame, head/hands, projection and overlay readback. |
| `view` | Head, eye transforms and projection tangents from SteamVR. |
| `menu` / `flick` / `camera` | Quick Menu, held Action Menu, stick-release selection, and the camera route described below. |
| `camera_mode` / `camera_fly` | Documented OSC mode 0-6 or Flying boolean; inspect the actual native UI after writing. |
| `overlays` | `overlay_keys`; SteamVR readback of existing handles, visibility, alpha, width and absolute transforms. Camera keys start `cc.orbiters.desktop.orbit.Camera`. |
| `stop` | Stops owned VR processes, restores the journal and releases the lock. |
| `restore` | Standalone recovery, including after the controlling app crashes. |

Inputs are `trigger`, `grip`, `stick_x`, `stick_y`, `trigger_touch`, `grip_touch`,
`stick_click`, `stick_touch`, `a`, `a_touch`, `b`, `b_touch`, `x`, `x_touch`,
`y`, `y_touch`, `menu`, `menu_touch`, `thumbrest`, `trigger_click`. Sticks use
−1..1; other inputs use 0..1. Thumbrest reports touch, without a fabricated
trackpad position. The controllers use SteamVR's installed Quest Pro render models
and Oculus Touch input profile, preserving the person's saved application bindings.

For VRChat's menu, a short B/Y press opens the Quick Menu. Hold it to operate the
Action Menu; the person's current binding closes that menu when the button is
released. Inspect an eye capture before selecting a menu item. The Action Menu's
Tools route opens the camera; writing OSC Mode is not evidence of opening it.

## Recovery

```powershell
python tools/vr-sandbox/sandbox.py restore
```

Recovery needs no MCP connection. The controller journals original file bytes
and absence, driver registration, SteamVR settings, chaperone configuration,
application configuration and Orbiters binding files before mutation. If the
development app is activated through the controller, its settings and MCP client
files are backed up too, and the installed app is restarted on restoration.
A separate watcher attempts recovery if the controller dies. Recovery refuses
foreign VR processes and retains the journal on failure; resolve that conflict
before running restore again. Successful restoration verifies backed-up file hashes.

Other installed drivers are disabled only in the sandbox's temporary settings.
This matters: the first local run crashed with installed pose-hook drivers loaded.
The virtual display component supplies display timing; a display-only emulated
HMD started VRChat but stalled composition. These changes do not alter the
smoothing driver's source or normal registration.

## Limits

Emulation verifies SteamVR composition and VRChat interaction, not optical comfort,
Virtual Desktop transport or delivery to physical Quest displays. Captures of the
two eyes are sequential; do not treat a moving pair as synchronized stereo ground
truth. The application itself and the camera attachment matrix require separate
live evidence. Never use a successful driver build, a passing unit test, or an
empty eye texture as proof of that workflow.

Driver ABI: [Valve OpenVR v2.12.14](https://github.com/ValveSoftware/openvr/tree/v2.12.14).
The vendored headers retain Valve's BSD license. The published
[virtual display interface](https://github.com/ValveSoftware/virtual_display)
documents the final composited display handoff.

## Tested bootstrap and camera route

The feedback directory contains the sandbox-ready handoff and inspected live
captures. Before starting, wait for the existing SteamVR lock to be released.
Use the installed bridge's existing authentication; do not print its credentials.
After starting the supervisor, activate a debug executable copied inside the
sandbox build root through the standalone controller:

```python
import sys
sys.path.insert(0, 'tools/vr-sandbox')
import sandbox
sandbox.remote({'action':'app', 'app_exe':
    'C:/Users/Enzo/.orbiters-build/vr-sandbox/orbiters-sandbox-desktop-next.exe'})
from client import Bench
b = Bench()
b.call(action='vrchat', world_id='wrld_b6ad492e-2e9f-415b-ab5e-b237cb4a0221')
b.call(action='camera', device='right')
b.call(action='capture', output_prefix='C:/Users/Enzo/.orbiters-build/vr-sandbox/check')
```

`menu` accepts quick/wheel/close (close releases the held wheel button, not a
Quick Menu toggle). `flick` accepts `stick:[x,y]` and recentres both axes atomically;
Enzo's binding selects on stick release. `camera` holds B/Y, flicks Tools then
Camera, and releases. Inspect the capture to confirm the actual UI state.
`camera_fly` accepts `enabled:true/false`; verify the OSC readback and picture.
`view` returns actual head, eye transforms and projection tangents. To grab the
camera, set `input:grip,value:1`, move that controller with `pose`, then release
grip with zero. These operate real VRChat; pose commands alone do not prove a grab.

The app activation journals its settings, bridge/client configuration and installed
service before replacement. Stop/restore returns the installed service. It is a
development bootstrap operation, not a publicly exposed MCP action.

## Current verification boundary

The sandbox has run real SteamVR, real VRChat in a signed-in strict private
instance, and the application's overlays in composited eye captures. Normal
restoration verified eleven journal entries; a supervisor crash test verified
seven SteamVR entries and lock release without MCP. This is bench validation,
not completion of the camera attachment acceptance matrix.

`camera_mode` accepts integer `mode` 0 through 6 after opening the camera through
the wheel. The tested client displays Photo, Stream, Animated (value 3),
Multilayer, Print and Drone. A mode write is a request; inspect the native label.
Before `stop` or standalone `restore`, read the lock and ensure it is your
session. These commands recover the current journal; never use them to terminate
another stream's live test. Wait for that stream to finish restoration.

The camera implementation and its observed failures remain in the feedback
capture directory. See `done2-sandbox-camera.md` there for the precise
implemented/tested boundary; sandbox readiness alone is not camera acceptance.

## Motion evidence

The motion probe queues native RGBA readbacks during sampling and encodes PNGs
only afterward. Its raw-buffer budget is 2 GiB; 48 pairs at the tested 1620x1800
resolution use about 1.1 GiB. Requested duration and frame count are targets:
measure actual QPC intervals from the manifest. The local dense run achieved
about 22 eye pairs/second. Eyes remain sequential, including during motion.

Activating an app waits for the installed service to quit. Wait for the bridge
and its signed-in account state before requesting VRChat. A successful launch
request is not proof of arrival: verify the strict private instance and zero
other users with `get_state`.

Pose quaternions are normalized. At a mathematically exact half-turn, a literal
zero scalar component is represented as `cos(pi/2)` to avoid a native VRChat hand
snap observed with the zero endpoint. This changes orientation below meaningful
double precision; it affects only this developer controller, never real tracking.

The supervisor enumerates process names with a Windows snapshot, then queries
creation times only for runtime processes. Snapshot failure is an error. Owned
processes are still identified by PID plus creation time, not names alone.
