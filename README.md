# Orbiters - N4N0

The companion app of **N4N0** for VRChat, and of its menu: fast walk, YouTube, calls, 3D pictures, music, the light and
your avatar's saved states, side by side as equals on the window's Home, in the SteamVR overlay and on N4N0's screen on
your avatar. It also
captures VRChat worlds while you play and brings them into Unity with [Mixer](https://github.com/Orbiters-cc/Mixer).

N4N0's capture kit on your avatar draws a small *capture strip* into VRChat's Stream camera: the exact camera pose,
projection and frame flags, bit-coded in grey blocks, plus *depth frames* that carry exact depth from the game. The app
drives it over OSC (contracts `Docs/rig-frames.md`, N4N0's capture kit); N4N0 › Record in the Action Menu starts and
stops a recording too.
Orbiters - N4N0 receives the Stream camera over Spout, decodes every frame, keeps lossless keyframes, poses and
depth, and writes a session bundle that the reconstruction (next milestone) turns into Gaussian splats and meshes.
It never asks for your VRChat login: it only reads VRChat's local log, OSC and Spout.

Orbiters - N4N0 also runs **N4N0**, the companion that floats on your avatar (the N4N0 prop from My Avatar's gallery):
it answers on a text panel everyone around you can read, puts your hoodie on when you ask, keeps your GoGoLoco pose
when you change avatar, holds parameters that worlds reset, shows an AFK message while you are away, saves the videos
a world plays and takes pictures. Its assistant runs on Orbiters (`/orb` on the website, free quota or your own key);
the window's N4N0 page turns it off for a tool without AI. You talk to it by holding Talk in its menu, touching it, the
Talk button in the window or the SteamVR tab, or by starting a sentence with "Nano" (N4N0 said aloud); speech is transcribed on this
computer and only the sentences meant for N4N0 go to the assistant.

Calling N4N0 also opens **N4N0's menu**: the SteamVR binding "Open N4N0's menu" (two quick taps on the left thumbrest or
trackpad, or a double press of X on controllers with neither, unless you change it in SteamVR's controller settings), the
watch on a wrist, the wake word on its own, or a hand on N4N0. It has fast walk (walk
boost on or off), YouTube (a keyboard with YouTube's suggestions as you type, five results a page with their thumbnail,
channel and duration, and "Copy link" to paste in a world's video player; a sentence you say on the search page is what
it searches), calls, a 3D picture with a timer, the music that plays, N4N0's light and the avatar's states. A copied link
can be dropped in the world as a card on N4N0's screen (Drop): for 15 seconds whoever grabs it copies the link, through
Orbiters for everyone else (contracts `Docs/orb.md`, Link card). The window's Home is the same
menu, with the keyboard for searching. It opens in the SteamVR overlay in front of you, and SteamVR's laser
works on it; when your orb has the projected screen ("Replace Orbiters overlay" on N4N0's card in My Avatar), it
opens on that screen in VRChat instead, where you touch it with your index finger and people around see it too
(contracts `Docs/orb.md`, Menu and Screen). YouTube is read without an account or a key, as YouTube's own website does.

A **3D picture** (1, 4 or 8 s) flies VRChat's camera around you while it records, then reconstructs the place. N4N0's
camera rig follows the camera and about every other frame shows six more views at once (left, front, right, back, up,
down; contracts `Docs/rig-frames.md`): the same flight gives the reconstruction several times the angles. Those views
have no depth of their own, and the rig renders the world six more times a frame, for the length of the picture only.
While you **record**, the rig follows VRChat's camera too and turns on for 0.3 s every few metres the camera goes (at
most every 4 s; Settings, "Six more views while recording"), so the reconstruction also sees what you forgot to film,
and N4N0's capture kit draws wherever the camera goes. In VR, what the recording has covered so far (the Capture page's
camera path) floats above a controller while you record (Settings, "Coverage while recording"); in desktop mode it
shows in a corner of the screen. Neither is ever in the recording: SteamVR draws overlays into the headset only, and
the frames are VRChat's camera's own picture, shared over Spout before anything is drawn over it.
With **camera dolly streams** on (Settings, experimental, VRC+), a 3D picture flies four cameras with VRChat's camera
dolly in its multi-stream mode: yours, and one left, one right and one above it (1.2 m off), all looking where yours
does; the dolly gives each extra camera a Spout sender of its own (`VRCSender2` to `VRCSender4`), which recordings then
read beside `VRCSender1`. Their frames that carry your capture kit's strip join the session as views of their own. In
VRChat, set Camera › Dolly › Streaming to Multi once (an import does not carry it).

In VR it also offers **walk boost**: while you push the stick you walk with, your play space moves along with you, so
you cover more ground in VRChat. The chaperone stays on your room's walls, and the play space goes back when you
change world or recenter. Its strength is a slider in the SteamVR tab, in Settings, and in N4N0's Walk boost radial on your avatar.
**States** keep how an avatar is: every parameter OSC can set (GoGoLoco's pose and the avatar's size only when the state
says so, N4N0's own never), saved under a name from Home's States page or N4N0's menu, and put back over OSC in one
press. One state per avatar can be its default, put back after each world join and each switch to that avatar. They
live in `avatar-states.json` beside the settings (contracts `Docs/orb.md`, Saved states).

And, when you turn it on in Settings, it shares **your frame rate per world** with Orbiters: a summary a minute with
the world, your avatar, how many people were there and the names of their avatars (never who they are), plus your
graphics card, processor and headset (contracts `Docs/world-performance.md`). The Map explorer shows which worlds run
well.

## Layout

| Path | What it is |
|---|---|
| `crates/orbiters-desktop` | The desktop app: a small tray service (controller, capture worker, VRChat watchers, OSC; no GPU) and, while it is open, the window in its own process (`--window`, egui on wgpu) talking to it over a loopback link |
| `crates/orbiters-ui` | Interface toolkit shared by the window and the coming VR overlay: theme, fonts, springs, the glow background shader, widgets, record button, trail map, toasts, offscreen renderer |
| `crates/orbiters-capture` | Capture worker process (`orbiters-capture`): frame sources, strip and depth decoding, session bundles |
| `crates/orbiters-reconstruct` | Reconstruction worker process (`orbiters-reconstruct`): trains Gaussian splats from a session's keyframes, poses and depth on the GPU (WGSL kernels on wgpu, checked against an f64 reference) |
| `crates/orbiters-sound` | Interface sounds computed at start-up (a port of React Bits' Sound Lab renderer, see its NOTICE.md), the default-output player and the settings' mic monitor |
| `crates/orbiters-orb` | N4N0, the companion on the avatar (contracts `Docs/orb.md`): its text panel's sender, avatar controls found by label, the tools and client of its assistant, the wake word, saved videos |
| `crates/orbiters-n4n0` | N4N0's runtime, shared with the Android app: the panel, faces and helpers, the assistant's turns, N4N0's menu on the avatar's screen and the menu host the interfaces run, saved states, link cards, its ears, the music watcher; what each platform does its own way behind small traits (the app, OSC, clipboard, browser, sounds, language, ears, mouse, music, files) |
| `crates/orbiters-voice` | N4N0's ears, a worker process (`orbiters-voice`): the microphone while N4N0 listens, Silero VAD and NVIDIA Parakeet through sherpa-onnx, on the CPU; it also installs the speech models |
| `crates/orbiters-call` | N4N0's calls, a worker process (`orbiters-call`): WebRTC voice (webrtc-rs, Opus) between two Orbiters - N4N0 apps, through Cloudflare Realtime TURN when needed (contracts `Docs/orb-calls.md`) |
| `crates/orbiters-audio` | Microphones and a process's sound through WASAPI, for the capture worker and the voice worker |
| `crates/orbiters-core` | Shared logic: settings, session bundles, keyframes, VRChat log, OSC/OSCQuery (this computer's VRChat, or a Quest's over the Wi-Fi), worker protocols; TLS, folders and the account's store per platform |
| `crates/orbiters-projects` | Unity discovery, shared VCC/ALCOM project storage and VPM packages, avatar inventory and Toolkit setup requests ([Unity projects](docs/unity-projects.md)) |
| `crates/orbiters-spout` | Spout2 receiver and test sender (Direct3D 11), protocol-compatible with the Spout2 SDK |
| `crates/orbiters-setup` | The setup (`OrbitersSetup-<version>.exe`): installs the app for the person in `%LOCALAPPDATA%\Programs` (no administrator rights), updates it and uninstalls it, in the app's look |
| `crates/orbiters-update` | Releases: the signed release file apps read, the setup's payload, the install record, downloading an update |
| `xtask/` | Release tooling, `cargo xtask keygen / bump / dist / publish / release` (see [Releasing](docs/releasing.md)) |
| `contracts/` | Submodule [OrbitersContracts](https://github.com/Orbiters-cc/OrbitersContracts): formats shared with Mixer |

## Building

Requirements: Rust (stable, MSVC toolchain on Windows). No web runtime: the interface is drawn natively.

```sh
git submodule update --init
cargo build --release -p orbiters-desktop -p orbiters-capture -p orbiters-reconstruct -p orbiters-voice -p orbiters-call
target/release/orbiters-desktop
```

Building `orbiters-voice` downloads sherpa-onnx's prebuilt shared libraries (about 8 MB, from k2-fsa's GitHub
releases) and places `sherpa-onnx-c-api.dll` and `onnxruntime.dll` next to it; ship them with it. The speech models
(about 490 MB) are not part of the app: the N4N0 page installs them into `%LOCALAPPDATA%/Orbiters/Models` when the
person asks. `orbiters-voice transcribe <file.wav>` checks them on 16 kHz recordings.

Building `orbiters-call` compiles the Opus codec from source and needs CMake on the PATH (3.x; the portable zip from
Kitware's releases is enough).

Ship `THIRD_PARTY_NOTICES.md` and `THIRD_PARTY_LICENSES.md` with the programs. After a dependency change, regenerate
the licenses with `python scripts/third_party_licenses.py`.

The app looks for `orbiters-capture`, `orbiters-reconstruct`, `orbiters-voice` and `orbiters-call` next to its own
executable. What stays running is small: in the tray the
service and the worker use about 25 MB together and no GPU. The window is a separate process that exists only
while it is open (about 120 MB, mostly the GPU driver); it repaints only while something moves, and looping
animations run only while it is focused. It draws on the graphics card (Settings, Interface, Window graphics, can
move it to the processor's integrated GPU), its 3D view's splats on the integrated GPU while VRChat runs (3D view on
the integrated GPU), and while it is behind VRChat it draws the service's news at most five times a second ([what that saves VRChat](docs/igpu-experiment.md)). `orbiters-desktop --hidden` starts in
the tray, `--quit` stops it.

`orbiters-desktop --gpu-bench <scenario> [--gpu integrated|graphics-card]` opens the window over a stand-in controller
and drives it, for measuring what it costs each GPU with the tools in `scripts/gpu-lab` (see
[docs/igpu-experiment.md](docs/igpu-experiment.md)).

To review the interface without a window, render it offscreen in its main states (capture idle and recording,
consent notice, toasts, sessions, session detail, settings, splash):

```sh
target/release/orbiters-desktop --screenshot <out folder> [--sessions <sessions folder>] [--raw <raw captures folder>] [--scale 1.5] [--only sessions]
```

`--only e2e` instead records a real test pattern through the capture worker, driven by synthetic clicks (Test
pattern, then the record button), and writes the session into the sessions folder. `--only e2e-reconstruct` opens
the newest session and presses Reconstruct (the real worker replaces its `reconstruction/` folder). `--only splats-3d`
opens the newest session's 3D view (the splats drawn by the reconstruction's own rasteriser on the window's GPU),
then orbits it.

## Releasing

Bump the version, commit, push, then `cargo xtask release`: it builds and signs the programs on this computer, packs
them into the setup, publishes it on orbiters.cc and tags the commit. Installed apps find the new version within
hours, download it in the background and install it on their next restart. Everything about it, first-time set-up
included, is in [docs/releasing.md](docs/releasing.md).

## Command-line tools

```sh
cargo run -p orbiters-capture -- list-sources
cargo run -p orbiters-capture -- send-test --name "Orbiters Test Pattern"           # a Spout sender with valid strips
cargo run -p orbiters-capture -- record --source "spout:Orbiters Test Pattern" --seconds 10
cargo run -p orbiters-capture -- record --source "raw:<raw capture folder>"          # replay a Mixer simulator capture
cargo run -p orbiters-capture -- send-test --name Dolly2 --stream 1                  # another camera's view of the same frames
cargo run -p orbiters-capture -- record --source spout:Dolly1 --extra Dolly2,Dolly3   # several senders as one session
cargo run -p orbiters-capture -- verify "<raw capture folder>"                        # compare with its ground truth
cargo run --release -p orbiters-reconstruct -- "<session folder>" [--iterations N] [--budget N] [--width N]
cargo run -p orbiters-desktop -- --orb-menu <folder>    # N4N0's screen pages and layout table (for Toolkit), sample pages
cargo run -p orbiters-desktop -- --orb-menu <folder> --theme <theme.json> --strings <strings.json>   # in a theme and language
cargo run -p orbiters-desktop -- --orb-menu-strings <file.json>    # the menu's English texts by key, with each key's max
```

`--orb-menu` writes `N4N0 screen pages.png` and `N4N0 screen layout.png`, which Orbiters Toolkit carries as
`Editor/VRChat/OrbScreenPages.png.bytes` and `OrbScreenLayout.png.bytes`: run it and copy them over whenever the
menu's layout or look changes, so the avatar's screen matches the overlay. With `--theme` it writes only that theme's
pages (as `N4N0 screen pages.png`), the layout and `N4N0 screen colors.json`; `--strings` draws the pages' texts in
another language. The base themes and translations are `crates/orbiters-orb/menu-themes/` and
`crates/orbiters-orb/menu-strings/` (contracts `Docs/orb.md`, Menu themes and translations); write `en.json` again with
`--orb-menu-strings` when a text or a place changes (a test says so).

## Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## License

Free to use, proprietary source: see [LICENSE.md](LICENSE.md), [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
Copyright (c) 2026 Enzo DUTRA / blackorbit.
