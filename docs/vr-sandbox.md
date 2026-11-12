# Developer VR sandbox

The development sandbox runs real SteamVR and VRChat with an emulated headset
and two `oculus_touch` controllers. It is separate from the tracking smoothing
driver and is not shipped in the installer. Its driver refuses activation without
a shared-memory session owned by a live controller process.

This work is still being validated. The feedback directory's `sandbox-ready.md`
is the readiness handoff; this guide alone is not a completion claim.

## Build and start

From this worktree, use Windows Python with `psutil` and Visual Studio C++ tools:

```powershell
& tools/vr-sandbox/build.ps1
python tools/vr-sandbox/sandbox.py start
```

The controller stays running. In another terminal, or an agent tool call, use
`python tools/vr-sandbox/sandbox.py request '<JSON>'`. PowerShell callers can use
Python's `sandbox.remote({...})` directly to avoid shell JSON quoting.

Build Rust through the shared `deskcargo.sh` queue, with its existing target.
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
