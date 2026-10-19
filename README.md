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
| `app/` | The desktop app: Tauri v2 shell (`app/src-tauri`, Rust) and its interface (`app/src`, React + Motion + Tailwind) |
| `crates/orbiters-capture` | Capture worker process (`orbiters-capture`): frame sources, strip and depth decoding, session bundles |
| `crates/orbiters-core` | Shared logic: settings, session bundles, keyframes, VRChat log, OSC/OSCQuery, worker protocol |
| `crates/orbiters-spout` | Spout2 receiver and test sender (Direct3D 11), protocol-compatible with the Spout2 SDK |
| `contracts/` | Submodule [OrbitersContracts](https://github.com/Orbiters-cc/OrbitersContracts): formats shared with Mixer |

## Building

Requirements: Rust (stable, MSVC toolchain on Windows), Node.js 20+, and on Windows the WebView2 runtime (part of
Windows 11).

```sh
git submodule update --init
cd app
npm install
npm run tauri dev        # builds the capture worker, starts Vite and the app
npm run tauri build      # release build and installer
```

`npm run dev` alone serves the interface in a browser with a built-in simulator (no native app needed), which is
how the interface is designed and reviewed.

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
cd app && npm run typecheck
```

## License

Free to use, proprietary source: see [LICENSE.md](LICENSE.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Copyright (c) 2026 Enzo DUTRA / blackorbit.
