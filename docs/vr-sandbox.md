# Developer VR sandbox

The development sandbox runs real SteamVR and VRChat with an emulated headset
and two `oculus_touch` controllers. It is separate from the tracking smoothing
driver and is not shipped in the installer. Its driver refuses activation without
a shared-memory session owned by a live controller process.

The feedback directory's `sandbox-ready.md`
is the readiness handoff; this guide alone is not a completion claim.

## Shared session: take a turn, never restart VRChat yourself

SteamVR and VRChat start once and stay up; agents take turns on them. Starting and
closing VRChat for every experiment costs minutes of loading and is what preceded
Easy Anti-Cheat's "Launch Error / EAC unload timeout" (see below). Use these
commands, from your worktree, with your stream name as owner:

```powershell
# 1. Take a turn (waits in line; starts SteamVR if no session runs). Your test exe must be inside
#    C:/Users/Enzo/.orbiters-build/vr-sandbox; its logs go to --logs (default <sandbox>/logs, never Enzo's).
python tools/vr-sandbox/sandbox.py acquire --owner vr1113b/shell --app C:/Users/Enzo/.orbiters-build/vr-sandbox/orbiters-sandbox-shell5.exe
# 2. Drive it: through your exe's bridge (vr_sandbox tool, client.Bench), or directly:
python tools/vr-sandbox/sandbox.py call --owner vr1113b/shell '{"action":"capture","output_prefix":"C:/Users/Enzo/.orbiters-build/vr-feedback-1111/x/a"}'
# 3. Give the turn back as soon as you have your pictures (before building, before reading long logs):
python tools/vr-sandbox/sandbox.py release --owner vr1113b/shell
```

- **VRChat**: if `status` (or `acquire`'s answer) shows `vrchat` running, use it;
  `vrchat` is refused while one runs. When none runs, `vr_sandbox` `vrchat` from
  your exe's bridge launches it (once EAC is idle). VRChat cannot be moved to
  another instance without a relaunch: a link sent to the running game only
  opens its world page and waits for a click (tested 2026-11-13). For another
  world or a fresh spawn point use `acquire --cold`.
- **Release** stops only your exe (its `--quit`, then a kill), then puts the
  session back to neutral: every input released, head and hands at their start
  poses, OSC camera mode 0 and flying off, and every VRChat menu closed, checked
  in VRChat's log (`Immobile status changed to`; a short left Y press toggles the
  menus). The avatar stays where it is: `avatar_moved_since_launch` in the
  answers says whether a turn walked it with the stick. Enzo's installed Orbiters
  is stopped once per session and comes back at its end.
- **Your turn ends by itself** after 10 minutes without any request from you
  (an agent cut off mid-turn): the controller stops your exe, resets to neutral
  and gives the session to the next in line. Every request renews the turn;
  `renew --owner` does too. A request after that fails with "Not your turn".
- **The session ends by itself** 10 minutes after the last turn ended: VRChat is
  closed cleanly, SteamVR stopped, every journalled file restored and Enzo's
  installed app restarted.
- **Restart only when needed**: `acquire --cold` (a test that needs a fresh
  SteamVR/VRChat) or `--render 2112x2304` / `--render default` when the emulated
  eye size differs (the driver reads `render-target.txt` once, when SteamVR loads
  it, so a size change restarts SteamVR and VRChat; never edit that file while a
  session runs). A VRChat that crashed is noticed (`vrchat_exited` in the log);
  launch it again with `vrchat`. Everything else (your exe, its logs folder,
  poses, worlds) changes within a turn.
- **Enzo wins**: the session gives SteamVR back to him at once when he runs
  `python tools/vr-sandbox/sandbox.py yield`, creates the file
  `C:/Users/Enzo/.orbiters-build/vr-sandbox/YIELD`, or his headset connects to
  Virtual Desktop (a streamer connection to a local-network device for 10 s).
  The turn ends, VRChat closes cleanly, his SteamVR files and app are restored,
  and `acquire` waits while his own SteamVR runs. Him using the mouse and
  keyboard is not a reason to stop: he steps away when he sees you need control.
- `status` shows the holder, the line, the session's render size, VRChat and the
  last turns (also in `shared-session.json`). `restore` is for recovery when the
  controller died; it refuses while another agent holds a turn.
- Older scripts (`sandbox.py start`/`stop` from a branch without this) see the
  shared session's lock and must wait like before; the lock keeps its keys
  (`session`, `pid`, `stream`, `time`) plus `shared: true`. MCP `start` takes a turn
  for the program's owner, MCP `stop` ends that turn only.

## Easy Anti-Cheat ("Launch Error / EAC unload timeout")

The dialog is a window of `start_protected_game.exe` (EAC's bootstrapper); no
VRChat.exe starts. The one occurrence (2026-11-13 12:37:54) came 82 s after a
VRChat that quit 10 s after it started, during "Attempting to login to EOS",
before "AntiCheat Session Begin"; the EAC driver had unloaded at once and the next
launch six minutes later worked. The controller therefore:

- never closes a VRChat before its anti-cheat session began (or 90 s), closes it
  with its window's close (as its own button does) and kills it only after 30 s;
- before every launch waits until no `VRChat.exe`, `start_protected_game.exe`,
  `launch.exe` or `EasyAntiCheat_EOS.exe` is left, the `EasyAntiCheat_EOS`
  service is STOPPED and the `EasyAntiCheat_EOSSys` driver service is deleted, for
  5 s (up to 180 s, then a clear error, never a blind launch). Each launch
  re-creates that driver service and it is deleted about 2 s after VRChat exits
  (sampled); EAC's error 30005 behind this dialog is "CreateService failed", so a
  driver service still present, even stopped, is the state to wait out;
- closes a leftover Launch Error dialog (a bootstrapper window older than 45 s
  without its VRChat child), with a picture of it, and retries a failed launch once;
- logs every launch, close, turn and EAC service/process change with timestamps
  in `C:/Users/Enzo/.orbiters-build/vr-sandbox/eac-log.jsonl`. After an
  occurrence, read the lines around `launch_error_window`: they hold the service
  states, processes and the driver's load/unload events at that moment.

A `STOP_PENDING` EasyAntiCheat_EOSSys that never stops is reported by VRChat
players to need a reboot; the controller then refuses to launch and says so.

## Build

From this worktree, use Windows Python with `psutil` and Visual Studio C++ tools:

```powershell
& tools/vr-sandbox/build.ps1
& tools/vr-sandbox/build-motion.ps1
```

Build Rust through the shared `deskcargo.sh` queue, with its existing target.
Release your turn before compiling. Do not delete another stream's build artifacts
(or the sandbox exes another stream may still use) to make space.
The sandbox bridge adapter exists only in debug builds and only when
`ORBITERS_VR_SANDBOX_ROOT` points to this worktree's `tools/vr-sandbox` directory.
The normal bridge's authentication and feature gate remain in effect.

A new session refuses existing VR processes and takes
`C:/Users/Enzo/.orbiters-build/vr-sandbox/steamvr.lock` exclusively before changing
anything. Do not remove another stream's lock. No headset or VRChat process that
the sandbox did not launch may be stopped.

## MCP control

The existing `orbiters-vrchat` MCP server exposes `vr_sandbox` in the enabled
development process. Its actions are:

| Action | Arguments and result |
| --- | --- |
| `start` | Takes a turn for the program's owner (starting the shared session if none runs). |
| `status` | Recorded processes and commanded head/hand states. |
| `vrchat` | `world_id`; creates a strict private instance using the bridge's signed-in account; starts VRChat through its EAC launcher (without `--no-vr`) only when none runs and EAC is idle, otherwise sends the instance to the running VRChat. |
| `home` / `neutral` | Rejoin the session's home instance / release inputs, reset poses and camera, and rejoin. |
| `pose` | `device`: head/left/right, `position`: Standing metres, `rotation`: quaternion **w,x,y,z**, optional `duration_s` 0–30. A duration interpolates smoothly at 144 updates/second. |
| `input` | `device`: left/right, `input`, `value`; optional `duration_s` releases it after a bounded press. Otherwise release explicitly with zero. |
| `capture` | `output_prefix`; saves `-left.png` and `-right.png` from SteamVR's composited eye textures, including overlays. |
| `motion_capture` | `device`, optional target `position`/`rotation`, `duration_s`, `frames` 3-80, `output_prefix`; timed native eye PNG pairs plus JSONL with QPC readback intervals, scene frame, head/hands, projection and overlay readback. |
| `view` | Head, eye transforms and projection tangents from SteamVR. |
| `menu` / `flick` / `camera` | Quick Menu, held Action Menu, stick-release selection, and the camera route described below. |
| `camera_mode` / `camera_fly` | Documented OSC mode 0-6 or Flying boolean; inspect the actual native UI after writing. |
| `overlays` | `overlay_keys`; SteamVR readback of existing handles, visibility, alpha, width and absolute transforms. Camera keys start `cc.orbiters.desktop.orbit.Camera`. |
| `stop` | Ends this program's turn (neutral reset); the session stays up. |
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
captures. Use the installed bridge's existing authentication; do not print its
credentials. Take a turn with your debug executable copied inside the sandbox
build root, then drive VRChat through its bridge:

```python
import sys
sys.path.insert(0, 'tools/vr-sandbox')
import sandbox
sandbox.acquire_turn('vr1113b/shell', app='C:/Users/Enzo/.orbiters-build/vr-sandbox/orbiters-sandbox-shell5.exe')
from client import Bench
b = Bench()                     # requests from the exe's bridge carry its turn automatically
b.call(action='vrchat', world_id='wrld_b6ad492e-2e9f-415b-ab5e-b237cb4a0221')   # only if status shows no VRChat
b.call(action='camera', device='right')
b.call(action='capture', output_prefix='C:/Users/Enzo/.orbiters-build/vr-sandbox/check')
sandbox.remote({'action': 'release'}, owner='vr1113b/shell')
```

`menu` accepts quick/wheel/close (close releases the held wheel button, not a
Quick Menu toggle). `flick` accepts `stick:[x,y]` and recentres both axes atomically;
Enzo's binding selects on stick release. `camera` holds B/Y, flicks Tools then
Camera, and releases. Inspect the capture to confirm the actual UI state.
`camera_fly` accepts `enabled:true/false`; verify the OSC readback and picture.
`view` returns actual head, eye transforms and projection tangents. To grab the
camera, set `input:grip,value:1`, move that controller with `pose`, then release
grip with zero. These operate real VRChat; pose commands alone do not prove a grab.

The first app of a session journals Enzo's settings, bridge/client configuration and
stops his installed service; the end of the session returns them. It is a
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
Standalone `restore` recovers the current journal; never use it (or `--force`) to
end another stream's turn. `release` ends your own turn only.

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
The log-backed `in_world` state can become true at `Joining`, while the eyes
still show `Connecting...`. Before camera input, wait for `Finished entering
world.` after the exact instance identifier in the current VRChat output log,
then inspect the eye picture. Reusing an instance also risks stale log state;
the current process's completed load and actual pixels are the readiness proof.

Pose quaternions are normalized. At a mathematically exact half-turn, a literal
zero scalar component is represented as `cos(pi/2)` to avoid a native VRChat hand
snap observed with the zero endpoint. This changes orientation below meaningful
double precision; it affects only this developer controller, never real tracking.

The supervisor enumerates process names with a Windows snapshot, then queries
creation times only for runtime processes. Snapshot failure is an error. Owned
processes are still identified by PID plus creation time, not names alone.

Recovery journal replacement retries Windows access/sharing errors 5, 32 and 33
up to twenty attempts, 50 ms apart. A concurrent reader can briefly deny delete
sharing. The previous journal stays intact until replacement succeeds; a
persistent error still fails and the temporary file is removed. This does not
relax ownership checks or treat a failed write as a successful recovery.

The journalled sandbox settings disable `dashboard.enableDashboard` for the
owned test session. SteamVR otherwise sometimes opens its dashboard over VRChat
and intercepts the camera wheel. The original settings bytes, including the
owner's dashboard preference, are restored with the other journal entries.

Recovery ignores processes confirmed dead by zombie status, zero threads or zero handles; live foreign processes still prevent changes. App activation allows 90 seconds for the bounded installed-service shutdown and launch sequence.
