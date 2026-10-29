# Upload preparation speed (2026-10-29)

Scope: local preparation in `controller/maps.rs::web_preview`, called by Map explorer's
`Controller::upload_preview` and Photos & captures' `controller/shares.rs::share`.
The other two callers are ignored tests that upload; the new ignored
`upload_prep_timings` test never contacts a server.

## Method

Release builds only, with `CARGO_TARGET_DIR=C:/Users/Enzo/.orbiters-target` and
`CARGO_INCREMENTAL=0`. Timings include PLY reading, start-view selection, web splat
encoding, recorded cameras, GPU creation, PC/Quest tree construction, every tune
depth's targets/400 steps, serialization, sky lookup, and total preparation.
`ORBITERS_PREP_TIMING=1` prints machine-readable `upload_prep` records. Optional
`ORBITERS_PREP_PROFILE=1` uses the existing GPU timestamp profiler; it waits after
every step and is measured separately from normal elapsed times.

The sky stage only finds the existing reconstruction's `sky.png`; it does not
generate or encode a sky. JPEG conversion belongs to the later upload request.
PC/Quest already tune once when PC fits Quest; coarse is the top of tuned Quest
and is never tuned separately. Tree moments and bottom-up boxes already use rayon.
The GPU already keeps pipelines, splat buffers, and a bounded image-size cache.

Inputs under `C:/Users/Enzo/Documents/Orbiters/Sessions`:

| Session | Reconstructed splats |
| --- | ---: |
| Exoplanet Journey 2026-10-22 16-31-54 | 44,565 |
| Canine 2026-10-20 14-27-07 | 202,578 |
| New Generation / 新世代 2026-10-21 15-54-31 | 970,138 |

Hardware: Intel Core i7-12700K, RTX 4080, Intel UHD 770; NVIDIA driver
32.0.16.1692 and Intel 32.0.101.7088. No VRChat, SteamVR server, or reconstruction
worker was running at the initial process check. Existing desktop apps remain open.

## Evidence

Initial baseline (unchanged algorithm with timing instrumentation):

| Session | Read | Web | GPU setup | Tree build | PC tune | Quest tune | Serialize | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Exoplanet | 0.010 | 0.011 | 0.665 | 0.056 | 4.831 | shared | 0.027 | 5.657 |
| Canine | 0.020 | 0.048 | 0.304 | 0.147 | 11.030 | shared | 0.066 | 11.684 |
| New Generation | 0.094 | 0.371 | 0.290 | 1.063 | 12.496 | 10.087 | 0.230 | 24.747 |

All times in seconds. Start-view/camera/metadata/sky work accounts for the remaining
time. Detailed depth measurements and repeated/final runs follow below.

## Background preparation decision

Do not start speculative background GPU tuning in this change. The reconstruction
worker explicitly asks for low GPU priority and `train` attaches the SteamVR
governor; `web_preview` uses the default device without that pacing. The existing
iGPU setting concerns the interactive 3D viewer. `docs/igpu-experiment.md` measured
shared-memory contention from that iGPU too. Moving upload preparation to the
reconstruction-completion hook without a scheduler and contention measurements
would not establish that it leaves VRChat's frames unaffected.
