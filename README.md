# Orbiters desktop

Capture VRChat worlds while you play and bring them into Unity with [Mixer](https://github.com/Orbiters-cc/Mixer).

A shader on your capture avatar draws a small *capture strip* into VRChat's Stream camera: the exact camera pose,
projection and frame flags, bit-coded in grey blocks, plus *depth frames* that carry exact depth from the game.
Orbiters desktop receives the Stream camera over Spout, decodes every frame, keeps lossless keyframes, poses and
depth, and writes a session bundle that the reconstruction (next milestone) turns into Gaussian splats and meshes.
It never asks for your VRChat login: it only reads VRChat's local log, OSC and Spout.

## Layout

| Path | What it is |
|---|---|
| `crates/orbiters-desktop` | The desktop app: a native window (egui on wgpu) over a controller that owns the capture worker, watches VRChat and listens to OSC; keeps running from the tray |
| `crates/orbiters-ui` | Interface toolkit shared by the window and the coming VR overlay: theme, fonts, springs, the glow background shader, widgets, record button, trail map, toasts, offscreen renderer |
| `crates/orbiters-capture` | Capture worker process (`orbiters-capture`): frame sources, strip and depth decoding, session bundles |
| `crates/orbiters-core` | Shared logic: settings, session bundles, keyframes, VRChat log, OSC/OSCQuery, worker protocol |
| `crates/orbiters-spout` | Spout2 receiver and test sender (Direct3D 11), protocol-compatible with the Spout2 SDK |
| `contracts/` | Submodule [OrbitersContracts](https://github.com/Orbiters-cc/OrbitersContracts): formats shared with Mixer |

## Building

Requirements: Rust (stable, MSVC toolchain on Windows). No web runtime: the interface is drawn natively.

```sh
git submodule update --init
cargo build --release -p orbiters-desktop -p orbiters-capture
target/release/orbiters-desktop
```

The app looks for `orbiters-capture` next to its own executable. The interface repaints only while something
moves; looping animations run only while the window is focused, so it costs nothing while you play.

To review the interface without a window, render it offscreen in its main states (capture idle and recording,
consent notice, toasts, sessions, session detail, settings, splash):

```sh
target/release/orbiters-desktop --screenshot <out folder> [--sessions <sessions folder>] [--raw <raw captures folder>] [--scale 1.5] [--only sessions]
```

`--only e2e` instead records a real test pattern through the capture worker, driven by synthetic clicks (Test
pattern, then the record button), and writes the session into the sessions folder.

## Command-line tools

```sh
cargo run -p orbiters-capture -- list-sources
cargo run -p orbiters-capture -- send-test --name "Orbiters Test Pattern"           # a Spout sender with valid strips
cargo run -p orbiters-capture -- record --source "spout:Orbiters Test Pattern" --seconds 10
cargo run -p orbiters-capture -- record --source "raw:<raw capture folder>"          # replay a Mixer simulator capture
cargo run -p orbiters-capture -- verify "<raw capture folder>"                        # compare with its ground truth
```

## Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## License

Free to use, proprietary source: see [LICENSE.md](LICENSE.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Copyright (c) 2026 Enzo DUTRA / blackorbit.
