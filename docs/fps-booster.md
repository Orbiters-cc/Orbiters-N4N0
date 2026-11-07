# The FPS booster

One slider: **Off · Background · Picture · Avatars**, in Settings and in the overlay's All tools, sharing one state. Each step
includes the ones before it. It has been built and tested without a headset: nothing here has been measured in VR yet, and no
number is promised.

| Step | What it spends | It really does | It does not |
|---|---|---|---|
| Off | Nothing | The frame logger runs; nothing else changes | |
| Background | Other programs' speed | Limited by the processor, with allowlisted programs using a core or more: compares putting them in **efficiency mode** (Windows' EcoQoS, Task Manager's "Efficiency mode"), A/B/A/B, and keeps that only if real frames rise. Stops preparing shares. Paces Orbiters' panels at 30 a second when the graphics card is short or the limit unknown | Close, suspend or kill anything; change priority classes; touch anything off the list; slow a program that is playing or recording sound; undo a choice someone made by hand |
| Picture | Sharpness, waiting for 3D pictures | 3D pictures wait until VRChat leaves or the headset is off a minute. Limited by the graphics card: compares `supersampleScale` × 0.85, keeps it only if real frames rise and the card's time really fell; 3D panels go to Light | Lower sharpness when the processor is the limit: it says so instead |
| Avatars | How other people look | A list, one item at a time, of VRChat's own settings for the person to change in VRChat. Done compares 30 s before with 30 s after, labelled as what it is | Change any of them itself. Nothing of VRChat's is written |

## Why efficiency mode and not Below Normal

A program started by a Below Normal one starts Below Normal too: Windows hands the priority class on. A lowered browser's new tabs,
and anything started from a lowered editor's terminal, would be changed without being recorded, and could never all be put back.
A browser link or the VRChat SDK's Build & Test can start VRChat itself, which the booster may never touch. Efficiency mode is not
handed on (`crates/orbiters-booster/tests/inheritance.rs` shows both on a real Windows), so the booster changes exactly the
processes it records. On a processor with efficiency cores Windows runs these programs there; without them, the effect is probably
smaller. Either way it is measured before it is kept.

## The hard line

Nothing in, on or around VRChat's process. `orbiters_booster::programs` is the one place that reads or changes another program, and
before opening anything it refuses:

- a name on the never-list: VRChat, `start_protected_game`, EasyAntiCheat, everything starting with `vr` or `steam`, the VR runtimes
  and streamers, recorders, voice, tracking and OSC tools, Orbiters;
- any PID that a fresh list of the system's processes does not show with the same name and creation time, on every path, recovery
  and the sign-in restorer included.

Listing processes opens none of them. A process that passed is then opened **read-only**, and its image name and creation time are
checked before anything else is read. Only while that checked handle is held, which keeps its PID from going to another process, is
a second handle asked for with the right to change it. A restore can only ever take efficiency mode off.

**What remains:** if a program ends between the list and that first open and the system gives its PID to another process, VRChat or
its anti-cheat included, that process gets one read-only look at its image name and creation time, and nothing more: no right to
change it, no token, no other read. No Windows API opens a process other than by its PID, so this can't be closed further; it is the
access Task Manager and game overlays use on VRChat all the time.

**Sound.** A program with an active audio session, playing or recording (read from Windows' audio sessions, `IAudioSessionManager2`,
which opens no process), is never put in efficiency mode, and is put back as soon as it starts. It goes by name: a browser whose audio
service plays is left whole, its tabs included. This is checked at every pass, every 5 seconds, so before each test too, and what
plays sound doesn't count as competition a test could act on. If the audio sessions can't be read, nothing is slowed.

**Choices made by hand.** A program someone takes out of efficiency mode (Task Manager, the program itself), or a sharpness someone
puts back in SteamVR, is theirs until the step is changed: the booster doesn't set it again.

**Exactly back.** A program's power throttling is read and journaled whole, timer resolution's bit included; efficiency mode is
added to it, and what was there is what comes back.

The allowlist is exact names of browsers, editors and compilers: `chrome`, `msedge`, `firefox`, `brave`, `opera`, `code`, `devenv`,
`rider64`, `Unity`, `blender`, `rustc`, `cargo`, `link`, `cl`, `msbuild`. Left out on purpose: Python, Node, .NET and Docker (they
host trackers, face tracking, OSC and voice tools), `ffmpeg` (recorders), Claude and Codex (Orbiters' own bridge runs them).

SteamVR: only `steamvr/supersampleScale` and `steamvr/supersampleManualOverride` are ever read or written, whatever a journal says.
VRChat's registry is read, never written, for the list's current values.

## Restoration

`%LOCALAPPDATA%\Orbiters\booster\journal.json`. Each change is written `intended` and flushed before it is made, with its original
value, or that it was absent. Restoring compares the value now with what was written: still ours comes back, already back is
dropped, anything else was changed by someone and stays. **A failed read is none of these**: an entry stays until a restore is read
back as done. See `orbiters_booster::journal` and `orbiters_booster::steamvr`.

| Event | What happens |
|---|---|
| Off, VRChat loses the scene, another world | At once. The level is remembered |
| The overlay exits, or SteamVR quits normally | SteamVR's values come back live before it goes, and SteamVR saves them; the service puts programs back as the overlay's reports stop |
| The overlay crashes | The next overlay restores SteamVR's values before it does anything else |
| **SteamVR crashes** | Nothing is restored through a SteamVR that is gone. The entries and `RunOnce` stay. SteamVR had saved the booster's values in its file, so: with SteamVR closed the service edits `steamvr.vrsettings` (one key, every other byte kept); if SteamVR starts again first, the next overlay finds the booster's values there and puts back what the person had |
| **The service crashes** | **A program in efficiency mode stays so until the service's next start, or until it exits.** This gap is real. SteamVR's values are the overlay's |
| Power loss | Efficiency mode is gone with the processes. While the journal has entries a `RunOnce` value runs `orbiters-desktop --booster-restore` at the next sign-in |
| Uninstall | Recovery runs after the app closes. With SteamVR open the window asks to close it and waits two minutes; past that, or silently, the journal and `RunOnce` stay with a copy of the uninstaller beside the journal, which deletes itself once nothing is owed, and the uninstall still finishes. With nothing owed the journal goes, and removing data removes the booster's logs |
| App update | Recovery runs at the service's start, before anything new is applied. A journal written by a newer version is not read, and never replaced by an older copy |

The journal is outside `%LOCALAPPDATA%\Orbiters\Desktop` on purpose: removing data at an uninstall does not delete what is owed.
A journal that can't be read, nor its copy, is left as it is, and the booster changes nothing and says so. A restore that settles
nothing writes nothing; a SteamVR value owed while SteamVR stays closed is tried again once a minute.

## The logs

`%LOCALAPPDATA%\Orbiters\logs`, on this computer only, never uploaded, separate from world performance statistics.

- `fps-booster.jsonl`: one line each 5 s while VRChat has the scene, at every level. Goes to `.1` at 20 MB. Names no person and no
  world: a count of people, and a number that only says "another world".
- `fps-frames-<date>.csv`: each frame, only during a comparison; the last 7 days are kept.
- A line with `self_check` when VRChat leaves: if real frames differ from `refresh × (1 − reprojected share)` as SteamVR's compositor
  counted them by more than 10%, the logger is what is wrong.

Virtual Desktop's and other streamers' frame synthesis happens outside SteamVR and does not show: real frames stay honest, and
"filled in" means SteamVR's only.

## Every threshold is a guess

5% gain, 25% fewer stalls, 5% agreement, 15 s blocks, 1 core of competition, the classifier's table: starting values for a test to
correct. They are named constants in `orbiters_booster::{classify, trial, engine}`.

## Not known yet

- Whether SteamVR applies `supersampleScale` to VRChat live (test A), and whether Auto's `GpuSpeed.gpuSpeedRenderTargetScale` is in
  the same unit as `supersampleScale`: compare SteamVR's shown resolution before and during a "with" block.
- What efficiency mode gains beside VRChat, on this PC's hybrid processor and on others.
- Trials don't know about a person's own recording in OBS or ShareX: only Orbiters' capture stops them.
