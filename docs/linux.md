# Linux

Orbiters - N4N0 on Linux is being ported in streams (foundation first). It is not released, and nothing about VRChat has been tried
on a real Linux computer with Steam: do not tell anyone it works there.

## What it can do

`crates/orbiters-core/src/here.rs` is the one list: `HERE.<capability>` for this build, and `Here::WINDOWS`, `Here::MAC`, `Here::LINUX`.
A feature, page, setting or worker asks it by name, never "is it a Mac" or "is it Windows". Linux says only what its code does: a stream
turns its capability on when its code lands, and the pages follow. Windows has every one (a test says so). `here::LINUX` is for words
and looks alone.

## Where its code is

| File | What | Stream |
|---|---|---|
| `orbiters-core/src/proton.rs` | Steam's roots, VRChat's library and Proton prefix, Windows paths in it, its registry file (read only) | done |
| `orbiters-core/src/processes.rs` | the processes from `/proc`, without their threads, with their whole names | done |
| `orbiters-core/src/dpapi.rs` (`sealing`) | the session store's key and seal, shared with the Mac's unsigned builds | done |
| `orbiters-core/src/disk.rs` | a disk's free space | done |
| `orbiters-desktop/src/controller/linux_vrchat.rs` | VRChat for Home and Where: what runs (done), starting it | L1 |
| `orbiters-desktop/src/linux/{tray,startup,open}.rs` | tray, start with the session, opening and revealing files | L2 |
| `orbiters-desktop/src/linux/{media,audio_devices}.rs`, `orbiters-sound/src/linux.rs`, `orbiters-audio/src/linux.rs` | sound, microphones, what plays | L3 |
| `orbiters-openvr/src/linux.rs` | SteamVR's client (the structures' layouts are in place) | L4 |
| `orbiters-desktop/src/linux/coverage.rs` | the coverage panel over the game | L7 |
| `orbiters-desktop/src/linux/update.rs`, `PLATFORM_FOLDER` and `linux_image_name` in `orbiters-update` | updates | L12 |

Everything else keeps its `cfg(not(windows))` arm, which does nothing or says why it cannot.

Two traps: `sysinfo` counts every thread as a process on Linux unless asked `.without_tasks()`, and Wine's threads are named
`VRChat.exe`; and OpenVR packs `VREvent_t` and `Compositor_FrameTiming` to 4 bytes there.

The VRChat bridge is off on Linux (`bridge/caps.rs` refuses every tool) and nothing sends input to the game.

## Checking

From Windows, for both triples (`c_char` is signed on one and not on the other), tests included, nothing linked:

    scripts/linux/check.sh setup      once: H:/n4n0-linux-t0 (N4N0_LINUX_T0), removed to undo
    scripts/linux/check.sh both       every crate Linux builds
    scripts/linux/check.sh mac -p orbiters-desktop --tests      what a change to shared code does on a Mac

On the Raspberry Pi (arm64 Debian 12; it runs other things too: never build there but through this): everything but `orbiters-desktop`,
which needs more memory than it has.

    git push ssh://pi/home/orbit/n4n0-linux/n4n0.git HEAD:refs/heads/<branch>
    git -C contracts push ssh://pi/home/orbit/n4n0-linux/contracts.git HEAD:refs/heads/main
    N4N0_JOBS=1 scripts/linux/pi-bg.sh <stream> <branch> test --locked --no-fail-fast -p orbiters-core -- --test-threads=2
    scripts/linux/pi-wait.sh <stream>

`scripts/linux/pi-build.sh` is what it runs there (`~/n4n0-linux/bin/n4n0-build`): one build at a time, no swap, a memory cap, stopped when
the Pi has little memory left. `~/n4n0-linux/UNDO.txt` says how everything is removed. The heavy crates (core, projects, reconstruct)
want one job and one crate a turn.
