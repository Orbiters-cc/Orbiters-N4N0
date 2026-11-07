# The FPS booster

One slider: **Off · Background · Picture · Avatars**, in Settings and in the overlay's All tools, sharing one state. Each step
includes the ones before it. It has been built and tested without a headset: nothing here has been measured in VR yet, and no
number is promised.

| Step | What it spends | It really does | It does not |
|---|---|---|---|
| Off | Nothing | The frame logger runs; nothing else changes | |
| Background | Other programs' speed | Limited by the processor, with allowlisted programs using a core or more: compares lowering them to Below Normal, A/B/A/B, and keeps that only if real frames rise. Stops preparing shares. Paces Orbiters' panels at 30 a second when the graphics card is short or the limit unknown | Close, suspend or kill anything; touch anything off the list |
| Picture | Sharpness, waiting for 3D pictures | 3D pictures wait until VRChat leaves or the headset is off a minute. Limited by the graphics card: compares `supersampleScale` × 0.85, keeps it only if real frames rise and the card's time really fell; 3D panels go to Light | Lower sharpness when the processor is the limit: it says so instead |
| Avatars | How other people look | A list, one item at a time, of VRChat's own settings for the person to change in VRChat. Done compares 30 s before with 30 s after, labelled as what it is | Change any of them itself. Nothing of VRChat's is written |

## The hard line

Nothing in, on or around VRChat's process. `orbiters_booster::programs` is the one place that changes a priority, and it refuses
by name before opening anything: VRChat, `start_protected_game`, EasyAntiCheat, everything starting with `vr` or `steam`, the VR
runtimes, Orbiters. Listing processes opens none of them. VRChat's registry is read, never written, for the list's current values.

The list of what may be lowered is exact names: see `programs::ALLOWLIST`.

## Restoration

`%LOCALAPPDATA%\Orbiters\booster\journal.json`. Each change is written `intended` and flushed before it is made, with its original
value, or that it was absent. Restoring compares the value now with what was written: still ours comes back, already back is
dropped, anything else was changed by someone and stays. See `orbiters_booster::journal`.

| Event | What happens |
|---|---|
| Off, VRChat loses the scene, another world | At once. The level is remembered |
| Overlay exits | Restores SteamVR's values on the way out; the service puts priorities back as its reports stop |
| Overlay crashes | The next overlay restores SteamVR's values before it does anything else |
| Service crashes | **A lowered program stays lowered until the service's next start, or until it exits.** This gap is real. SteamVR's values are the overlay's |
| SteamVR quits or crashes | At the quit event if there is one; otherwise the service edits `steamvr.vrsettings`, one key, once `vrserver` and `vrmonitor` are gone |
| Power loss | Priorities are gone with the processes. While the journal has entries a `RunOnce` value runs `orbiters-desktop --booster-restore` at the next sign-in |
| Uninstall | Recovery runs after the app closes. With SteamVR open the window asks to close it and waits two minutes; past that, or silently, the journal and `RunOnce` stay with a copy of the uninstaller beside the journal, and the uninstall still finishes |
| App update | Recovery runs at the service's start, before anything new is applied. The format has a version, and a newer one is not guessed at |

The journal is outside `%LOCALAPPDATA%\Orbiters\Desktop` on purpose: removing data at an uninstall does not delete what is owed.
A journal that can't be read, nor its copy, is left as it is, and the booster changes nothing and says so.

## The logs

`%LOCALAPPDATA%\Orbiters\logs`, on this computer only, never uploaded, separate from world performance statistics.

- `fps-booster.jsonl`: one line each 5 s while VRChat has the scene, at every level. Goes to `.1` at 20 MB. Names no person and no
  world: a count of people, and a number that only says "another world".
- `fps-frames-<date>.csv`: each frame, only during a comparison.
- A line with `self_check` when VRChat leaves: if real frames differ from `refresh × (1 − reprojected share)` as SteamVR's compositor
  counted them by more than 10%, the logger is what is wrong.

Virtual Desktop's and other streamers' frame synthesis happens outside SteamVR and does not show: real frames stay honest, and
"filled in" means SteamVR's only.

## Every threshold is a guess

5% gain, 25% fewer stalls, 5% agreement, 15 s blocks, 1 core of competition, the classifier's table: starting values for a test to
correct. They are named constants in `orbiters_booster::{classify, trial, engine}`.
