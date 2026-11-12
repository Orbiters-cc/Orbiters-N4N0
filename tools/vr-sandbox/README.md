# Developer VR sandbox

This directory is development tooling, outside the installer and independent of
`crates/orbiters-smooth-driver`. It must never register a driver persistently.

Integration points inspected at base dc1ef14b:

- `crates/orbiters-desktop/src/bridge/{mcp,tools,win}.rs`: existing authenticated,
  owner-gated `orbiters-vrchat` server. The current launcher forces `--no-vr`.
- `crates/orbiters-openvr`: runtime client, eye readback and overlay operations.
- `crates/orbiters-smooth-driver`: server provider ABI and real-device smoothing;
  sandbox devices will use their own provider without modifying that driver.
- `crates/orbiters-desktop/src/overlay/orbit/{camera_frames,camera_track,vr}.rs`:
  image recognition, stereo recovery and fitted/provisional placement.

Runtime changes require exclusive ownership of
`C:/Users/Enzo/.orbiters-build/vr-sandbox/steamvr.lock`, no pre-existing VRChat or
vrserver process, and an on-disk recovery journal before changing configuration.
Restore must work without the MCP server. Existing user files must be restored as
bytes, including absent files remaining absent. A foreign VR process is never
terminated. Driver enablement requires a live sandbox session.

The OpenVR driver header in `vendor` is Valve's v2.12.14 header, BSD-3-Clause;
its license is retained beside it. It is used to compile the development driver
against the published C++ ABI rather than inventing server vtable slots.

The real SteamVR/VRChat bench has been exercised; this file is not a readiness
claim. Only `sandbox-ready.md` in the feedback evidence directory declares a
working sandbox after a real compositor run.
