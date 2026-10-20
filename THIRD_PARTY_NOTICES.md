# Third-party notices

Orbiters desktop is built on the following components. Each remains under its own license; the full texts ship
with the components (Cargo registry sources) and are available from each project.

## Rust crates

| Component | License |
|---|---|
| egui, eframe, egui-wgpu, egui-winit, epaint, emath, ecolor | MIT OR Apache-2.0 |
| wgpu, naga | MIT OR Apache-2.0 |
| winit | Apache-2.0 |
| AccessKit | MIT OR Apache-2.0 |
| tray-icon, muda | MIT OR Apache-2.0 |
| rfd | MIT |
| tiny-skia | BSD-3-Clause |
| png | MIT OR Apache-2.0 |
| bytemuck | Zlib OR Apache-2.0 OR MIT |
| pollster | Apache-2.0 OR MIT |
| raw-window-handle | MIT OR Apache-2.0 OR Zlib |
| winresource (build tool, not shipped) | MIT |
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

## Fonts

| Component | License |
|---|---|
| Phosphor Icons 2.1 (regular and fill), embedded; files from the egui-phosphor crate (MIT OR Apache-2.0) | MIT, see `crates/orbiters-ui/assets/Phosphor-LICENSE.txt` |
| egui's fallback fonts (epaint_default_fonts): Ubuntu Light, Hack, Noto Emoji, emoji-icon-font | Ubuntu Font Licence 1.0, MIT / Bitstream Vera, SIL Open Font License 1.1, MIT |
| Segoe UI Variable, Segoe UI, Cascadia Mono, Consolas (Windows) or the system's sans fonts (Linux) | Read from the system at run time, not distributed |

## Protocols and formats implemented from their specifications

| Specification | Notes |
|---|---|
| Spout2 (Lynn Jarvis / leadedge, BSD-2-Clause) | Orbiters desktop implements the Spout2 sender/receiver protocol (shared memory names and layout, access mutex, frame counter) in Rust, written from the published protocol; no Spout2 SDK code is included. Spout is a trademark of its authors. |
| QOI image format (Dominic Szablewski) | Implemented from the specification (qoiformat.org). |
| OSC 1.0 and OSCQuery | OSC messages through rosc; the OSCQuery service follows the OSCQuery proposal used by VRChat. |

VRChat is a trademark of VRChat Inc. Orbiters desktop is not affiliated with or endorsed by VRChat Inc.
