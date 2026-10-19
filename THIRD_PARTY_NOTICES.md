# Third-party notices

Orbiters desktop is built on the following components. Each remains under its own license; the full texts ship
with the components (Cargo registry sources and `node_modules`) and are available from each project.

## Rust crates

| Component | License |
|---|---|
| Tauri, tauri-build, tauri-plugin-dialog, tauri-plugin-opener, tauri-plugin-single-instance | Apache-2.0 OR MIT |
| windows (windows-rs, Microsoft) | MIT OR Apache-2.0 |
| serde, serde_json | MIT OR Apache-2.0 |
| anyhow, thiserror | MIT OR Apache-2.0 |
| crossbeam-channel | MIT OR Apache-2.0 |
| uuid | Apache-2.0 OR MIT |
| time | MIT OR Apache-2.0 |
| dirs | MIT OR Apache-2.0 |
| flate2 | MIT OR Apache-2.0 |
| rosc | MIT OR Apache-2.0 |
| mdns-sd | Apache-2.0 OR MIT |
| sysinfo | MIT |
| trash | MIT |

## Web interface

| Component | License |
|---|---|
| React, React DOM | MIT |
| Motion (motion.dev) | MIT |
| Tailwind CSS | MIT |
| lucide-react icons | ISC |
| Inter typeface (@fontsource-variable/inter) | SIL Open Font License 1.1 (font), MIT (package) |
| @tauri-apps/api and plugins | Apache-2.0 OR MIT |
| Vite, TypeScript (build tools, not shipped) | MIT, Apache-2.0 |

## Protocols and formats implemented from their specifications

| Specification | Notes |
|---|---|
| Spout2 (Lynn Jarvis / leadedge, BSD-2-Clause) | Orbiters desktop implements the Spout2 sender/receiver protocol (shared memory names and layout, access mutex, frame counter) in Rust, written from the published protocol; no Spout2 SDK code is included. Spout is a trademark of its authors. |
| QOI image format (Dominic Szablewski) | Implemented from the specification (qoiformat.org). |
| OSC 1.0 and OSCQuery | OSC messages through rosc; the OSCQuery service follows the OSCQuery proposal used by VRChat. |

VRChat is a trademark of VRChat Inc. Orbiters desktop is not affiliated with or endorsed by VRChat Inc.
