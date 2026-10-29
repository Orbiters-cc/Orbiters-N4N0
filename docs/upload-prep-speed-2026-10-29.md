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
generate or encode a sky. The later multipart upload streams the original PNG.
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

## Changes and experiments

1. Reuse the exact leaf render for the exact same scaled camera across tune depths,
   separately for each tree (PC/Quest leaves can differ). This is a 64 MiB bounded
   cache with shared pixel buffers. It skips leaf uploads when every target is
   already present. Cameras, resolution, random-camera seed, learning rates and all
   400 steps are unchanged. The pre-cache trial measured 5.208 / 11.085 / 23.055 s.
2. When PC fits Quest, serialize PC once and clone the encoded bytes, rather than
   clone all Gaussian arrays and encode the same tree again. This is included in
   that trial; serialization fell from 0.027 / 0.066 s to approximately 0.018 /
   0.035 s for the shared-tree sessions. Separate Quest serialization is unchanged.
3. Persist the four prepared files and metadata in one checksummed, atomic cache
   in the reconstruction folder, shared by both upload entry points. Its fingerprint
   contains SHA-256 hashes of the PLY, frames, report and manifest. A source change,
   malformed/partial archive or checksum mismatch triggers fresh preparation. A
   fresh random key replaces bytes 20-23 in all three trees on every upload. Cache
   hits use no GPU. Writes are optional: read-only/full disks and fingerprinting
   errors leave preparation usable. CPU fallback output is not persisted. One
   preparation mutex prevents two in-process callers from tuning the same capture
   simultaneously. Sources are hashed again before committing the cache.

The tuner measures floating-point, unquantized parameters before file encoding.
Even the two unchanged baseline runs differed by up to 0.1252 dB at a high-PSNR
depth; the target-reuse trial differed by up to 0.1820 dB. This is not silently
declared below the 0.05 dB threshold. The added `serialized_upload_quality` ignored
test instead also compares **the actual exported files** (including colour and
rotation quantization) on identical recorded views against the same encoded leaves,
resetting tile-count estimates for each evaluation view. All 88 evaluated cuts of
the trial passed the 0.05 dB gate; worst changes by session were -0.01322, -0.04966
and -0.00628 dB. Tree nodes and leaf splats were identical. Final cache-miss files
are checked separately below. Cache hits are checked byte for byte, excluding the
fresh upload key; their quality change is exactly zero.

Considered but not implemented: fewer/lower-resolution views or early stopping
would alter optimization and require a broader quality study. Existing PC/Quest
sharing, coarse extraction, rayon tree work and GPU buffer/pipeline reuse were
already present. Web encoding was only 0.011 / 0.048 / 0.371 s in the baseline;
overlapping that work cannot remove the dominant training cost and was not
implemented. No training-quality setting was reduced.

## Background preparation decision

Do not start speculative background GPU tuning in this change. The reconstruction
worker explicitly asks for low GPU priority and `train` attaches the SteamVR
governor; `web_preview` uses the default device without that pacing. The existing
iGPU setting concerns the interactive 3D viewer. `docs/igpu-experiment.md` measured
shared-memory contention from that iGPU too. Moving upload preparation to the
reconstruction-completion hook without a scheduler and contention measurements
would not establish that it leaves VRChat's frames unaffected.

<!-- measured-results:start -->
## Final timings

Two independent baseline runs; one final uncontended cache-miss run; medians of three separate-process cache-hit runs. All values are preparation seconds, excluding writing benchmark copies to disk.

| Session | Baseline 1 | Baseline 2 | Final first prepare | Cache hit | First gain vs baseline mean | Cache gain vs baseline mean |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Exoplanet | 5.657 | 5.352 | 5.058 | 0.0121 | 1.09x | 455x |
| Canine | 11.684 | 11.825 | 11.235 | 0.0380 | 1.05x | 310x |
| New Generation | 24.747 | 24.428 | 23.381 | 0.1513 | 1.05x | 163x |

The first preparation improves modestly (about 4-8% less time against the mean baseline). The large win is a later public/private share or retry: about 99.4-99.8% less preparation time, with no GPU work. A cache miss is not instantaneous.

One cache-miss trial overlapped Rust compilation and took 5.483 / 11.369 / 35.704 s. It is retained in the JSON evidence as `excluded_contended`, but excluded from the comparison above. No compiler, VRChat or SteamVR server was running at the uncontended timing check. Existing desktop applications were left open.

### Stage breakdown and cache overhead

| Stage (seconds) | Exoplanet | Canine | New Generation |
| --- | ---: | ---: | ---: |
| cache fingerprint | 0.003060 | 0.009513 | 0.054533 |
| read result | 0.003040 | 0.012902 | 0.068357 |
| start view | 0.000466 | 0.000967 | 0.006946 |
| web splat | 0.010608 | 0.044522 | 0.394996 |
| session cameras | 0.000117 | 0.000156 | 0.000228 |
| gpu setup | 0.376034 | 0.295741 | 0.324529 |
| tree build pc | 0.051041 | 0.156717 | 0.740426 |
| tree build quest | shared / absent | shared / absent | 0.400867 |
| tune pc | 4.532610 | 10.570862 | 11.704748 |
| tune quest | shared / absent | shared / absent | 9.189822 |
| tree serialize | 0.017152 | 0.035887 | 0.231788 |
| sky | 0.000026 | 0.000022 | 0.000023 |
| cache write | 0.015466 | 0.040396 | 0.143616 |

Cache writes, including the second source fingerprint, cost 0.015-0.144 s in the final run. The cache checksum and all source hashes are still checked on every hit; a hit is not an mtime-only shortcut.

### GPU versus CPU

| Timestamp-profiled target-reuse run | GPU execution span (s) | Profiled wall total (s) |
| --- | ---: | ---: |
| Exoplanet | 4.803 | 10.526 |
| Canine | 9.906 | 17.697 |
| New Generation | 20.058 | 33.933 |

Profiling waits after each step and changes scheduling, so these wall times are **not** normal preparation performance. GPU spans include work/waits between passes inside each profiled step; they are not CPU time. Per-pass GPU totals and all depth GPU spans were captured by the existing profiler. The JSON records the depth spans. The Windows process counters across all three sessions (including test startup and saving output copies) were:

| Run | Process wall (s) | CPU user + kernel (s) |
| --- | ---: | ---: |
| baseline_repeat | 41.764 | 9.969 |
| reuse_only | 39.508 | 12.219 |
| final_cold | 39.833 | 11.016 |
| reuse_profile | 62.844 | 3.578 |

CPU time can overlap GPU work; subtracting GPU time from wall time would not measure CPU execution. CPU-only stage times are shown above.

## Final quality and the baseline noise check

The single-baseline final-file check found -0.05497 dB at Canine PC/Quest depth 9 and failed its 0.05 dB gate. The **unchanged** second baseline was -0.04015 dB at the same depth. This failure is retained in the evidence. We compare against the mean of the two independently saved baselines, without changing the 0.05 dB gate. No final run was selected/retrained to pass.

| Session | Worst exported-file change versus baseline mean (dB) | Best change (dB) | Evaluated cuts |
| --- | ---: | ---: | ---: |
| Exoplanet | -0.02252 | +0.00986 | 33 |
| Canine | -0.03489 | +0.04991 | 29 |
| New Generation | -0.00628 | +0.01687 | 26 |

Every final cut is within 0.05 dB of the two-run baseline mean. PC/Quest nodes and original leaf splats are identical; coarse nodes are identical, while coarse leaves are tuned stand-ins and naturally differ across training runs. Web splat bytes are unchanged. All three cache-hit repetitions reproduce all four prepared files exactly, excluding the fresh tree key (36 file comparisons); cached camera coordinates are identical at their original f32 precision. No GPU quality validation runs on a cache hit because the content is identical.

### Each tuned depth

The merged and retained scores below are the tuner's unquantized PSNR against its leaves; the exported delta uses the actual files and the two-run baseline mean. A retained score is max(merged, trained), because the tuner discards training that made a depth worse. Shared Quest trees do not run a second tuner. Depth times include targets, scoring, all 400 steps and accepted parameter readback.

#### Exoplanet, PC (also Quest)

| Depth | Baseline s | Final s | Merged dB | Baseline retained dB | Final retained dB | Exported delta dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.226 | 0.209 | 20.35919 | 27.29717 | 27.29749 | +0.00067 |
| 1 | 0.262 | 0.264 | 20.32912 | 27.04128 | 27.04097 | +0.00057 |
| 2 | 0.299 | 0.292 | 23.25065 | 28.98563 | 28.98512 | -0.00207 |
| 3 | 0.311 | 0.296 | 22.78071 | 29.17326 | 29.17301 | +0.00986 |
| 4 | 0.344 | 0.331 | 22.82427 | 29.13895 | 29.13976 | -0.00052 |
| 5 | 0.352 | 0.323 | 23.58390 | 30.28370 | 30.28210 | -0.00117 |
| 6 | 0.388 | 0.354 | 24.14638 | 32.51863 | 32.51861 | +0.00451 |
| 7 | 0.560 | 0.525 | 25.12406 | 35.17342 | 35.17457 | -0.00569 |
| 8 | 0.618 | 0.600 | 26.71536 | 36.95800 | 36.95147 | -0.00949 |
| 9 | 0.696 | 0.658 | 30.42560 | 40.64475 | 40.65457 | -0.00245 |
| 10 | 0.774 | 0.681 | 38.03218 | 47.92419 | 47.72181 | +0.00290 |

#### Canine, PC (also Quest)

| Depth | Baseline s | Final s | Merged dB | Baseline retained dB | Final retained dB | Exported delta dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.305 | 0.344 | 14.99298 | 23.28739 | 23.28739 | -0.00001 |
| 1 | 0.353 | 0.275 | 17.62957 | 23.63379 | 23.63379 | +0.00143 |
| 2 | 0.407 | 0.414 | 20.02558 | 23.42088 | 23.42097 | +0.00019 |
| 3 | 0.935 | 0.833 | 21.30278 | 24.59736 | 24.59752 | +0.00040 |
| 4 | 0.962 | 0.835 | 23.22988 | 26.33478 | 26.33365 | +0.00001 |
| 5 | 0.978 | 0.851 | 24.19752 | 27.33936 | 27.33910 | -0.00124 |
| 6 | 1.018 | 0.881 | 25.34940 | 29.56409 | 29.56360 | -0.00043 |
| 7 | 0.994 | 0.891 | 28.57864 | 35.36848 | 35.36863 | -0.01073 |
| 8 | 0.714 | 0.721 | 32.82601 | 40.27427 | 40.24658 | +0.02359 |
| 9 | 0.745 | 0.768 | 34.42322 | 43.73334 | 43.63748 | -0.03489 |
| 10 | 0.866 | 0.894 | 35.69547 | 46.35187 | 46.37224 | +0.04991 |
| 11 | 1.119 | 1.240 | 38.57161 | 49.24104 | 49.19341 | +0.00160 |
| 12 | 1.629 | 1.614 | 51.09698 | 51.09698 | 51.09698 | +0.00000 |

#### New Generation, PC

| Depth | Baseline s | Final s | Merged dB | Baseline retained dB | Final retained dB | Exported delta dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.362 | 0.394 | 15.74020 | 17.39927 | 17.39929 | +0.00000 |
| 1 | 0.401 | 0.371 | 16.56918 | 17.97513 | 17.97511 | -0.00001 |
| 2 | 0.441 | 0.380 | 16.75026 | 19.16375 | 19.16141 | -0.00086 |
| 3 | 0.520 | 0.390 | 17.30061 | 20.77774 | 20.76763 | +0.00110 |
| 4 | 0.543 | 0.409 | 17.92445 | 21.28437 | 21.26631 | -0.00628 |
| 5 | 0.681 | 0.550 | 19.32376 | 22.94225 | 22.94224 | -0.00000 |
| 6 | 0.846 | 0.755 | 21.01638 | 25.52891 | 25.52934 | +0.00129 |
| 7 | 1.108 | 1.036 | 22.54000 | 27.71455 | 27.71422 | +0.00177 |
| 8 | 1.636 | 1.652 | 24.53441 | 31.41667 | 31.41545 | -0.00307 |
| 9 | 2.738 | 2.601 | 28.53182 | 36.19545 | 36.26339 | +0.01687 |
| 10 | 3.185 | 3.132 | 37.47157 | 39.69382 | 39.66321 | +0.00188 |

#### New Generation, QUEST

| Depth | Baseline s | Final s | Merged dB | Baseline retained dB | Final retained dB | Exported delta dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.425 | 0.665 | 16.04822 | 17.44167 | 17.44167 | +0.00000 |
| 1 | 0.469 | 0.423 | 16.72443 | 18.12834 | 18.12838 | +0.00001 |
| 2 | 0.482 | 0.413 | 16.72621 | 19.02356 | 19.02356 | +0.00000 |
| 3 | 0.495 | 0.460 | 17.37882 | 21.06287 | 21.06287 | +0.00000 |
| 4 | 0.515 | 0.416 | 17.96236 | 21.95570 | 21.95570 | +0.00007 |
| 5 | 0.597 | 0.493 | 19.29894 | 23.66111 | 23.66108 | -0.00027 |
| 6 | 0.742 | 0.690 | 20.78103 | 25.71921 | 25.71897 | -0.00106 |
| 7 | 1.132 | 1.017 | 23.55344 | 28.65688 | 28.65465 | -0.00083 |
| 8 | 2.016 | 1.736 | 26.86678 | 34.24359 | 34.22345 | +0.00442 |
| 9 | 3.197 | 2.862 | 41.76323 | 41.76323 | 41.76323 | +0.00000 |

### Coarse exported files

| Session | Depth | Baseline mean dB | Final dB | Delta dB |
| --- | ---: | ---: | ---: | ---: |
| Exoplanet | 0 | 25.84647 | 25.84713 | +0.00067 |
| Exoplanet | 1 | 24.20179 | 24.20235 | +0.00057 |
| Exoplanet | 2 | 23.71569 | 23.71498 | -0.00071 |
| Exoplanet | 3 | 26.11924 | 26.12910 | +0.00986 |
| Exoplanet | 4 | 24.98512 | 24.98504 | -0.00008 |
| Exoplanet | 5 | 27.08048 | 27.07932 | -0.00117 |
| Exoplanet | 6 | 26.30256 | 26.30371 | +0.00116 |
| Exoplanet | 7 | 27.39966 | 27.39703 | -0.00263 |
| Exoplanet | 8 | 31.14915 | 31.14173 | -0.00742 |
| Exoplanet | 9 | 32.92800 | 32.92773 | -0.00027 |
| Exoplanet | 10 | 40.06581 | 40.04329 | -0.02252 |
| Canine | 0 | 23.50397 | 23.50396 | -0.00001 |
| Canine | 1 | 23.87022 | 23.87165 | +0.00143 |
| Canine | 2 | 23.38783 | 23.38802 | +0.00019 |
| New Generation | 0 | 18.10659 | 18.10659 | +0.00000 |
| New Generation | 1 | 17.70747 | 17.70748 | +0.00001 |
| New Generation | 2 | 17.07363 | 17.07363 | -0.00000 |
| New Generation | 3 | 18.22707 | 18.22707 | +0.00000 |
| New Generation | 4 | 19.02428 | 19.02434 | +0.00006 |

All measurements, individual target/step times, both baseline scores, CPU counters and byte checks are in [the machine-readable evidence](upload-prep-speed-2026-10-29.json). Full local logs, test binaries and output copies are under `C:/Users/Enzo/.orbiters-target/upload-prep-speed/`; they are benchmark artifacts, not a release to try.

## Validation and limits

- `cargo test --release -p orbiters-reconstruct -- --test-threads=1`: 101 passed (97 library + 4 binary), six ignored by default.
- `cargo test --release -p orbiters-desktop controller::shares::tests -- --test-threads=1`: four passed.
- `cargo test --release -p orbiters-desktop upload_prep::tests -- --test-threads=1`: two passed, covering content invalidation, exact bytes/fresh keys, corruption and truncation.
- The maps filter contains three ignored tests. The local timing harness was explicitly run on all three real sessions for cache misses and hits; the two real-server upload tests stayed ignored.
- The serialized-file quality test explicitly evaluates PC, Quest and coarse cuts. The single-baseline failure and baseline-noise measurements are recorded above; the final two-baseline gate is also run explicitly.
- `cargo fmt -p orbiters-reconstruct -p orbiters-desktop` was used; unrelated pre-existing formatting differences were restored. `git diff --check` passes.
- No UI interaction, real upload, live API/provider test, VRChat frame-impact test, Quest device test, background GPU tuning or release was performed. Network transfer and server processing occur after preparation and are outside the measured path; the original sky PNG is streamed unchanged.
- Results cover these three sessions on this RTX 4080; they do not establish performance/quality on every capture or GPU. One final uncontended cold run is not a performance confidence interval.

## Local commits

- `95fffd6`: measure preparation stages and render quality on real sessions.
- `b59e4b8`: reuse identical leaf renders and PC/Quest encodings.
- `1f489ed`: cache complete prepared uploads by source content for both sharing paths.
- The final evidence/validation commit contains this completed report, the JSON evidence and repeated-baseline support in the ignored quality test; its hash is included in the implementation handoff.

All changes are local on `main`. No version bump, tag, push or release was made.

## Reproduce

```powershell
$env:CARGO_TARGET_DIR='C:/Users/Enzo/.orbiters-target'
$env:CARGO_INCREMENTAL='0'
cargo test --release -p orbiters-desktop upload_prep_timings --no-run
# Use the release test executable printed by Cargo; the Python wrapper preserves UTF-8 logs.
python scripts/upload-prep-bench.py --binary <test-executable> --out <evidence-folder> <session-1> <session-2> <session-3>
# Repeat unchanged inputs for cache hits. For a fresh miss, remove only the generated
# reconstruction/upload-prep-v1.bin file in the chosen session, never a target folder.
# File-quality test inputs are the numbered files saved by the timing harness:
$env:ORBITERS_PREP_BASE='<baseline-1 files folder>'
$env:ORBITERS_PREP_BASE_REPEAT='<baseline-2 files folder>'
$env:ORBITERS_PREP_FINAL='<final files folder>'
cargo test --release -p orbiters-reconstruct --lib serialized_upload_quality --no-run
python scripts/upload-prep-bench.py --test serialized_upload_quality --binary <reconstruct-test-executable> --out <quality-folder> <session-1> <session-2> <session-3>
```

<!-- measured-results:end -->
