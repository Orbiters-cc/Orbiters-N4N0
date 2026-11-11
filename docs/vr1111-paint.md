# VR1111 paint - F11/F12 implementation and evidence

Measured merged code head: `9130d7518393c036de513b562c10fca6f8d0e9b4` on `vr1111/paint`. Main `d17dabc9d15c0c50ae6415ec5442f9e301bddb22` is an ancestor (merge `7b201c8d3005c4dee62f1fc728f443225d777128`).

**Partial result; acceptance remains blocked.** Ordinary-page painting is isolated from input on a bounded worker, cached pages restore their pixels and hit geometry on the next turn, and the single composed sheet is preserved. Full requirements are not declared complete: MenuHost/Worlds remain synchronous and exact full-replay frame hashes differ. No headset, upload or GPU measurement is claimed.

- F11 final 600-entry warm-scroll cadence: **56.70 frames/s**, against 45. Worst measured gallery input turn across the three sizes and warm/cold motions: **4.623 ms**, against 11.
- F12 worst measured switch input turn: **19.424 ms**, `switch-1-Worlds`, against 11. The tables below retain all slow samples.
- Exact raw-part parity: **702/783** common captured frames identical. This is an exact failure if those counts differ; no visual tolerance substitutes for the required unchanged hashes.


Measured status: see target table below. These are CPU harness measurements, not headset validation. No upload/GPU benchmark, installed-app launch, SteamVR/VRChat manipulation, push, release or version bump was performed.

## Before changes

Original 1760x990 measurements were recorded before implementation in paint-before-{60,600,6000}. The 60-entry steady-scroll raster phase was 53.508/56.775/57.957/59.238 ms (p50/p95/p99/max), versus 0.235/0.357/0.563/0.632 ms text/layout, 0.106/0.212/0.282/0.387 ms tessellation/textures, and 0.635/0.875/0.889/1.431 ms finish/copy. Raster dominated; this selected a bounded latest-only CPU worker with parallel independent raster rows. No strip-rendering rewrite was added.

The original baseline did not separately reset decoded thumbnails for a controlled cold-scroll/cold-fling matrix. That gap is explicit. Final controlled before/after below uses the same merged executable and corrected script: --sync serial reference versus normal worker, at the merged shell's 1408x792 page size. It includes the Recents and source-identity fixes in both modes, so it isolates pipeline execution rather than reproducing all old behavior. Original and merged timings must not be compared as if resolution were unchanged.

## Final merged measurement conditions

Intel Core i7-12700K, 12 cores/20 logical processors; opt-level-2 dev build through deskcargo, shared target and CARGO_INCREMENTAL=0. One logged replay plus one timing run per case. Steady scroll and fling each run with warm and reset in-memory thumbnails, at 60/600/6000 entries. The synthetic fixture has 60 unique 640x360 pictures, hardlinked to unique entry paths for larger lists. Cold resets decoded/egui thumbnails, not OS filesystem caches. The machine remained in normal use; other streams could compile. No samples are discarded for slow scheduling.

Input turn = set_open + follow/menu_turn/springs + hand actions + synchronous paint/copy/composition. Eleven-ms pacing sleep is excluded from input cost and included in cadence wall time. Actual controllers, walk, poses, D3D upload and headset delivery are absent. VR header/back and page are composed on CPU into the same single picture as OrbitVr; composition is measured in finish/copy. All frames accepted from a worker carry full-page damage because a skipped frame's incremental damage cannot safely describe the displayed image.

## Final before/after input turns

Each cell gives p50 / p95 / p99 / max milliseconds. No threshold is relaxed.

| Entries | Motion | Serial p50 | p95 | p99 | max | Worker p50 | p95 | p99 | max |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 60 | pictures-cold-ingestion-scroll | 37.435 | 40.034 | 41.885 | 44.329 | 2.146 | 2.720 | 3.236 | 3.796 |
| 60 | pictures-cold-ingestion-fling | 44.477 | 56.035 | 65.332 | 65.353 | 1.975 | 2.492 | 2.739 | 3.046 |
| 60 | qm-sheet-pictures-scroll | 58.336 | 73.884 | 80.893 | 82.207 | 2.132 | 2.627 | 3.651 | 3.888 |
| 60 | pictures-fling | 40.650 | 49.890 | 51.978 | 51.984 | 2.052 | 2.524 | 2.716 | 3.581 |
| 600 | pictures-cold-ingestion-scroll | 37.054 | 39.880 | 41.710 | 43.310 | 2.275 | 3.712 | 3.950 | 4.485 |
| 600 | pictures-cold-ingestion-fling | 37.230 | 39.152 | 41.227 | 41.897 | 2.877 | 3.922 | 4.278 | 4.623 |
| 600 | qm-sheet-pictures-scroll | 37.288 | 43.059 | 50.624 | 50.646 | 2.045 | 2.517 | 3.126 | 3.475 |
| 600 | pictures-fling | 37.503 | 40.638 | 42.990 | 45.592 | 1.993 | 2.419 | 2.733 | 3.052 |
| 6000 | pictures-cold-ingestion-scroll | 35.895 | 38.242 | 38.726 | 39.671 | 2.147 | 2.456 | 2.491 | 2.704 |
| 6000 | pictures-cold-ingestion-fling | 36.917 | 38.918 | 40.236 | 40.325 | 2.181 | 2.545 | 2.835 | 3.510 |
| 6000 | qm-sheet-pictures-scroll | 36.428 | 38.327 | 40.834 | 41.019 | 2.127 | 2.429 | 2.647 | 2.789 |
| 6000 | pictures-fling | 36.325 | 37.805 | 39.492 | 39.693 | 2.228 | 2.627 | 3.179 | 3.959 |

## Completed steady-scroll frames

| Entries | Serial frames/s | Worker frames | Active wall seconds | Worker frames/s |
|---:|---:|---:|---:|---:|
| 60 | 13.45 | 78 | 1.365 | 57.13 |
| 600 | 19.43 | 77 | 1.358 | 56.70 |
| 6000 | 20.10 | 73 | 1.367 | 53.39 |

## Per-phase steady-scroll timings

| Entries | Mode | Phase | p50 | p95 | p99 | max ms |
|---:|---|---|---:|---:|---:|---:|
| 60 | serial input | follow_ms | 0.110 | 0.265 | 1.453 | 2.186 |
| 60 | serial input | facts_items_ms | 0.046 | 0.114 | 0.378 | 2.149 |
| 60 | serial input | text_ms | 0.389 | 0.955 | 1.761 | 2.097 |
| 60 | serial input | tess_ms | 0.184 | 0.477 | 1.347 | 2.511 |
| 60 | serial input | raster_ms | 54.176 | 68.680 | 77.096 | 77.790 |
| 60 | serial input | finish_ms | 3.005 | 5.032 | 8.088 | 8.520 |
| 60 | worker input | follow_ms | 0.079 | 0.105 | 0.136 | 0.147 |
| 60 | worker input | facts_items_ms | 0.060 | 0.090 | 0.107 | 0.108 |
| 60 | worker input | text_ms | 0.064 | 0.114 | 0.166 | 0.188 |
| 60 | worker input | tess_ms | 0.037 | 0.057 | 0.093 | 0.125 |
| 60 | worker input | raster_ms | 0.039 | 0.055 | 0.065 | 0.089 |
| 60 | worker input | finish_ms | 1.549 | 1.932 | 3.006 | 3.114 |
| 60 | completed worker | worker_items_ms | 0.005 | 0.018 | 0.033 | 0.033 |
| 60 | completed worker | worker_text_ms | 0.258 | 0.386 | 0.573 | 0.573 |
| 60 | completed worker | worker_tess_ms | 0.103 | 0.148 | 0.163 | 0.163 |
| 60 | completed worker | worker_raster_ms | 10.714 | 14.555 | 15.383 | 15.383 |
| 60 | completed worker | worker_finish_ms | 0.792 | 1.033 | 1.283 | 1.283 |
| 600 | serial input | follow_ms | 0.082 | 0.120 | 0.267 | 0.331 |
| 600 | serial input | facts_items_ms | 0.032 | 0.051 | 0.120 | 0.203 |
| 600 | serial input | text_ms | 0.245 | 0.382 | 0.433 | 0.647 |
| 600 | serial input | tess_ms | 0.114 | 0.200 | 0.314 | 0.444 |
| 600 | serial input | raster_ms | 35.265 | 40.805 | 46.760 | 48.463 |
| 600 | serial input | finish_ms | 1.535 | 2.254 | 3.162 | 3.409 |
| 600 | worker input | follow_ms | 0.084 | 0.117 | 0.145 | 0.148 |
| 600 | worker input | facts_items_ms | 0.052 | 0.083 | 0.134 | 0.138 |
| 600 | worker input | text_ms | 0.063 | 0.108 | 0.184 | 0.187 |
| 600 | worker input | tess_ms | 0.036 | 0.059 | 0.091 | 0.095 |
| 600 | worker input | raster_ms | 0.040 | 0.053 | 0.066 | 0.073 |
| 600 | worker input | finish_ms | 1.525 | 1.970 | 2.546 | 2.594 |
| 600 | completed worker | worker_items_ms | 0.006 | 0.013 | 0.029 | 0.029 |
| 600 | completed worker | worker_text_ms | 0.241 | 0.389 | 0.528 | 0.528 |
| 600 | completed worker | worker_tess_ms | 0.101 | 0.163 | 0.194 | 0.194 |
| 600 | completed worker | worker_raster_ms | 10.799 | 13.410 | 14.357 | 14.357 |
| 600 | completed worker | worker_finish_ms | 0.771 | 0.980 | 1.310 | 1.310 |
| 6000 | serial input | follow_ms | 0.110 | 0.132 | 0.141 | 0.149 |
| 6000 | serial input | facts_items_ms | 0.037 | 0.044 | 0.051 | 0.088 |
| 6000 | serial input | text_ms | 0.241 | 0.334 | 0.395 | 0.435 |
| 6000 | serial input | tess_ms | 0.110 | 0.146 | 0.204 | 0.216 |
| 6000 | serial input | raster_ms | 34.516 | 36.467 | 38.468 | 38.958 |
| 6000 | serial input | finish_ms | 1.339 | 1.621 | 2.173 | 2.199 |
| 6000 | worker input | follow_ms | 0.125 | 0.171 | 0.201 | 0.225 |
| 6000 | worker input | facts_items_ms | 0.058 | 0.122 | 0.130 | 0.149 |
| 6000 | worker input | text_ms | 0.063 | 0.133 | 0.151 | 0.152 |
| 6000 | worker input | tess_ms | 0.037 | 0.082 | 0.108 | 0.122 |
| 6000 | worker input | raster_ms | 0.039 | 0.052 | 0.055 | 0.061 |
| 6000 | worker input | finish_ms | 1.539 | 1.858 | 1.919 | 1.939 |
| 6000 | completed worker | worker_items_ms | 0.008 | 0.017 | 0.043 | 0.043 |
| 6000 | completed worker | worker_text_ms | 0.238 | 0.401 | 0.429 | 0.429 |
| 6000 | completed worker | worker_tess_ms | 0.104 | 0.131 | 0.175 | 0.175 |
| 6000 | completed worker | worker_raster_ms | 10.595 | 11.501 | 12.913 | 12.913 |
| 6000 | completed worker | worker_finish_ms | 0.746 | 0.893 | 0.992 | 0.992 |

## Rapid switches, including A-B-A

| Entries | Page | Serial first turn ms | Worker first turn ms |
|---:|---|---:|---:|
| 60 | All | 20.661 | 3.241 |
| 60 | Pictures | 30.474 | 4.742 |
| 60 | Settings | 16.375 | 3.954 |
| 60 | Tracking | 8.353 | 3.729 |
| 60 | Mics | 11.079 | 3.427 |
| 60 | Booster | 8.788 | 2.085 |
| 60 | Talk | 11.119 | 2.302 |
| 60 | Instances | 10.928 | 3.729 |
| 60 | Results | 8.874 | 3.363 |
| 60 | Menu | 10.584 | 5.220 |
| 60 | Worlds | 50.964 | 18.760 |
| 60 | Pictures | 27.379 | 3.166 |
| 600 | All | 11.833 | 2.930 |
| 600 | Pictures | 19.391 | 3.970 |
| 600 | Settings | 10.049 | 3.421 |
| 600 | Tracking | 4.976 | 3.631 |
| 600 | Mics | 6.656 | 3.583 |
| 600 | Booster | 5.034 | 2.433 |
| 600 | Talk | 7.129 | 2.412 |
| 600 | Instances | 4.879 | 3.579 |
| 600 | Results | 5.219 | 3.975 |
| 600 | Menu | 8.194 | 5.650 |
| 600 | Worlds | 19.442 | 19.424 |
| 600 | Pictures | 14.605 | 3.460 |
| 6000 | All | 11.043 | 3.624 |
| 6000 | Pictures | 17.217 | 3.940 |
| 6000 | Settings | 8.856 | 3.498 |
| 6000 | Tracking | 4.532 | 3.502 |
| 6000 | Mics | 6.056 | 3.506 |
| 6000 | Booster | 4.690 | 2.103 |
| 6000 | Talk | 6.340 | 2.676 |
| 6000 | Instances | 4.800 | 3.703 |
| 6000 | Results | 4.278 | 3.692 |
| 6000 | Menu | 4.681 | 5.285 |
| 6000 | Worlds | 17.986 | 19.372 |
| 6000 | Pictures | 14.601 | 3.607 |

| Entries | Ordinary-page next-turn pictures identical to last captured valid page |
|---:|---:|
| 60 | 10/10 |
| 600 | 10/10 |
| 6000 | 10/10 |

## Targets

| Entries | Gallery maximum input ms | Switch maximum input ms | Worst switch |
|---:|---:|---:|---|
| 60 | 3.888 | 18.760 | switch-1-Worlds |
| 600 | 4.623 | 19.424 | switch-1-Worlds |
| 6000 | 3.959 | 19.372 | switch-1-Worlds |


## Rapid-switch first-turn phase distribution

All 24 first turns (12 cold/refresh switches, 12 visited switches) per size, with the opening action included. Worker phase rows use accepted native-page completions over the full switch steps, including their refresh/rest turns; they are separate work, not added to input-turn time. Per-page first-turn values are above; complete per-step histograms are in each case report.

| Entries | Mode/sample | Phase | p50 | p95 | p99 | max ms |
|---:|---|---|---:|---:|---:|---:|
| 60 | serial first turns | turn | 12.453 | 30.474 | 50.964 | 50.964 |
| 60 | serial first turns | facts_items_ms | 0.023 | 0.038 | 0.043 | 0.043 |
| 60 | serial first turns | text_ms | 1.012 | 4.721 | 4.779 | 4.779 |
| 60 | serial first turns | tess_ms | 0.201 | 0.627 | 1.252 | 1.252 |
| 60 | serial first turns | raster_ms | 5.935 | 24.266 | 45.014 | 45.014 |
| 60 | serial first turns | finish_ms | 3.918 | 8.230 | 9.746 | 9.746 |
| 60 | worker first turns | turn | 3.954 | 14.122 | 18.760 | 18.760 |
| 60 | worker first turns | facts_items_ms | 0.026 | 0.040 | 0.056 | 0.056 |
| 60 | worker first turns | text_ms | 0.221 | 1.762 | 3.689 | 3.689 |
| 60 | worker first turns | tess_ms | 0.058 | 0.289 | 0.333 | 0.333 |
| 60 | worker first turns | raster_ms | 1.413 | 6.127 | 16.485 | 16.485 |
| 60 | worker first turns | finish_ms | 1.897 | 7.217 | 9.322 | 9.322 |
| 60 | accepted switch refreshes | worker_items_ms | 0.004 | 0.008 | 0.009 | 0.009 |
| 60 | accepted switch refreshes | worker_text_ms | 0.223 | 2.819 | 4.474 | 4.474 |
| 60 | accepted switch refreshes | worker_tess_ms | 0.106 | 0.261 | 1.056 | 1.056 |
| 60 | accepted switch refreshes | worker_raster_ms | 1.206 | 10.589 | 12.711 | 12.711 |
| 60 | accepted switch refreshes | worker_finish_ms | 0.737 | 0.842 | 1.317 | 1.317 |
| 600 | serial first turns | turn | 10.049 | 19.391 | 19.442 | 19.442 |
| 600 | serial first turns | facts_items_ms | 0.017 | 0.030 | 0.038 | 0.038 |
| 600 | serial first turns | text_ms | 0.675 | 4.466 | 5.414 | 5.414 |
| 600 | serial first turns | tess_ms | 0.151 | 0.606 | 1.122 | 1.122 |
| 600 | serial first turns | raster_ms | 4.969 | 16.086 | 17.008 | 17.008 |
| 600 | serial first turns | finish_ms | 1.974 | 3.748 | 3.750 | 3.750 |
| 600 | worker first turns | turn | 4.148 | 12.075 | 19.424 | 19.424 |
| 600 | worker first turns | facts_items_ms | 0.024 | 0.039 | 0.053 | 0.053 |
| 600 | worker first turns | text_ms | 0.223 | 1.973 | 2.850 | 2.850 |
| 600 | worker first turns | tess_ms | 0.059 | 0.322 | 0.386 | 0.386 |
| 600 | worker first turns | raster_ms | 1.508 | 5.274 | 16.959 | 16.959 |
| 600 | worker first turns | finish_ms | 1.899 | 6.526 | 7.039 | 7.039 |
| 600 | accepted switch refreshes | worker_items_ms | 0.004 | 0.013 | 0.025 | 0.025 |
| 600 | accepted switch refreshes | worker_text_ms | 0.217 | 2.258 | 4.588 | 4.588 |
| 600 | accepted switch refreshes | worker_tess_ms | 0.110 | 0.324 | 1.051 | 1.051 |
| 600 | accepted switch refreshes | worker_raster_ms | 1.030 | 11.072 | 12.327 | 12.327 |
| 600 | accepted switch refreshes | worker_finish_ms | 0.765 | 0.839 | 0.951 | 0.951 |
| 6000 | serial first turns | turn | 8.856 | 17.730 | 17.986 | 17.986 |
| 6000 | serial first turns | facts_items_ms | 0.015 | 0.032 | 0.065 | 0.065 |
| 6000 | serial first turns | text_ms | 0.634 | 3.993 | 4.793 | 4.793 |
| 6000 | serial first turns | tess_ms | 0.134 | 0.788 | 1.083 | 1.083 |
| 6000 | serial first turns | raster_ms | 4.834 | 14.874 | 15.900 | 15.900 |
| 6000 | serial first turns | finish_ms | 1.680 | 3.190 | 3.349 | 3.349 |
| 6000 | worker first turns | turn | 3.940 | 11.830 | 19.372 | 19.372 |
| 6000 | worker first turns | facts_items_ms | 0.029 | 0.039 | 0.051 | 0.051 |
| 6000 | worker first turns | text_ms | 0.208 | 2.086 | 2.687 | 2.687 |
| 6000 | worker first turns | tess_ms | 0.061 | 0.312 | 0.337 | 0.337 |
| 6000 | worker first turns | raster_ms | 1.373 | 5.516 | 16.809 | 16.809 |
| 6000 | worker first turns | finish_ms | 1.875 | 7.535 | 8.159 | 8.159 |
| 6000 | accepted switch refreshes | worker_items_ms | 0.008 | 0.013 | 0.021 | 0.021 |
| 6000 | accepted switch refreshes | worker_text_ms | 0.227 | 2.496 | 4.669 | 4.669 |
| 6000 | accepted switch refreshes | worker_tess_ms | 0.146 | 0.256 | 1.135 | 1.135 |
| 6000 | accepted switch refreshes | worker_raster_ms | 1.138 | 11.814 | 12.866 | 12.866 |
| 6000 | accepted switch refreshes | worker_finish_ms | 0.815 | 1.115 | 1.359 | 1.359 |

## Cold-scroll and fling phase distributions

Warm steady-scroll phases are above. These rows complete all requested gallery motions. Input phases are per turn; completed-worker phases include only accepted frames. Facts/items are measured directly; the enclosing follow/paint timings and any uncategorized bookkeeping remain in raw CSVs.

| Entries | Motion | Mode/sample | Phase | p50 | p95 | p99 | max ms |
|---:|---|---|---|---:|---:|---:|---:|
| 60 | pictures-cold-ingestion-scroll | serial input | facts_items_ms | 0.031 | 0.039 | 0.043 | 0.044 |
| 60 | pictures-cold-ingestion-scroll | serial input | text_ms | 0.253 | 0.376 | 0.470 | 0.513 |
| 60 | pictures-cold-ingestion-scroll | serial input | tess_ms | 0.114 | 0.210 | 0.262 | 0.294 |
| 60 | pictures-cold-ingestion-scroll | serial input | raster_ms | 35.351 | 37.853 | 39.498 | 42.325 |
| 60 | pictures-cold-ingestion-scroll | serial input | finish_ms | 1.559 | 1.883 | 2.069 | 2.147 |
| 60 | pictures-cold-ingestion-scroll | worker input | facts_items_ms | 0.048 | 0.076 | 0.118 | 0.119 |
| 60 | pictures-cold-ingestion-scroll | worker input | text_ms | 0.061 | 0.112 | 0.172 | 0.186 |
| 60 | pictures-cold-ingestion-scroll | worker input | tess_ms | 0.038 | 0.069 | 0.082 | 0.156 |
| 60 | pictures-cold-ingestion-scroll | worker input | raster_ms | 0.040 | 0.054 | 0.064 | 0.075 |
| 60 | pictures-cold-ingestion-scroll | worker input | finish_ms | 1.639 | 2.166 | 2.631 | 2.904 |
| 60 | pictures-cold-ingestion-scroll | completed worker | worker_items_ms | 0.004 | 0.006 | 0.009 | 0.009 |
| 60 | pictures-cold-ingestion-scroll | completed worker | worker_text_ms | 0.248 | 0.338 | 0.406 | 0.406 |
| 60 | pictures-cold-ingestion-scroll | completed worker | worker_tess_ms | 0.109 | 0.232 | 0.435 | 0.435 |
| 60 | pictures-cold-ingestion-scroll | completed worker | worker_raster_ms | 10.713 | 13.396 | 14.583 | 14.583 |
| 60 | pictures-cold-ingestion-scroll | completed worker | worker_finish_ms | 0.823 | 1.001 | 1.530 | 1.530 |
| 60 | pictures-cold-ingestion-fling | serial input | facts_items_ms | 0.038 | 0.056 | 0.107 | 0.270 |
| 60 | pictures-cold-ingestion-fling | serial input | text_ms | 0.303 | 0.467 | 0.581 | 0.612 |
| 60 | pictures-cold-ingestion-fling | serial input | tess_ms | 0.157 | 0.229 | 0.261 | 0.326 |
| 60 | pictures-cold-ingestion-fling | serial input | raster_ms | 41.595 | 51.968 | 59.331 | 62.291 |
| 60 | pictures-cold-ingestion-fling | serial input | finish_ms | 2.416 | 3.338 | 5.367 | 6.860 |
| 60 | pictures-cold-ingestion-fling | worker input | facts_items_ms | 0.050 | 0.083 | 0.097 | 0.207 |
| 60 | pictures-cold-ingestion-fling | worker input | text_ms | 0.062 | 0.119 | 0.149 | 0.389 |
| 60 | pictures-cold-ingestion-fling | worker input | tess_ms | 0.036 | 0.054 | 0.073 | 0.096 |
| 60 | pictures-cold-ingestion-fling | worker input | raster_ms | 0.039 | 0.050 | 0.065 | 0.083 |
| 60 | pictures-cold-ingestion-fling | worker input | finish_ms | 1.479 | 1.940 | 2.146 | 2.504 |
| 60 | pictures-cold-ingestion-fling | completed worker | worker_items_ms | 0.004 | 0.009 | 0.016 | 0.024 |
| 60 | pictures-cold-ingestion-fling | completed worker | worker_text_ms | 0.244 | 0.332 | 0.399 | 0.527 |
| 60 | pictures-cold-ingestion-fling | completed worker | worker_tess_ms | 0.102 | 0.206 | 0.254 | 0.340 |
| 60 | pictures-cold-ingestion-fling | completed worker | worker_raster_ms | 10.561 | 12.912 | 13.942 | 16.021 |
| 60 | pictures-cold-ingestion-fling | completed worker | worker_finish_ms | 0.725 | 0.994 | 1.176 | 1.827 |
| 60 | pictures-fling | serial input | facts_items_ms | 0.033 | 0.055 | 0.077 | 0.080 |
| 60 | pictures-fling | serial input | text_ms | 0.259 | 0.433 | 0.594 | 0.598 |
| 60 | pictures-fling | serial input | tess_ms | 0.121 | 0.227 | 0.433 | 0.783 |
| 60 | pictures-fling | serial input | raster_ms | 38.271 | 47.022 | 47.860 | 48.328 |
| 60 | pictures-fling | serial input | finish_ms | 1.701 | 2.966 | 3.184 | 3.468 |
| 60 | pictures-fling | worker input | facts_items_ms | 0.051 | 0.084 | 0.102 | 0.176 |
| 60 | pictures-fling | worker input | text_ms | 0.064 | 0.109 | 0.144 | 0.164 |
| 60 | pictures-fling | worker input | tess_ms | 0.036 | 0.053 | 0.062 | 0.088 |
| 60 | pictures-fling | worker input | raster_ms | 0.039 | 0.052 | 0.077 | 0.087 |
| 60 | pictures-fling | worker input | finish_ms | 1.534 | 2.001 | 2.184 | 2.770 |
| 60 | pictures-fling | completed worker | worker_items_ms | 0.005 | 0.010 | 0.023 | 0.029 |
| 60 | pictures-fling | completed worker | worker_text_ms | 0.245 | 0.351 | 0.404 | 0.408 |
| 60 | pictures-fling | completed worker | worker_tess_ms | 0.102 | 0.124 | 0.172 | 0.240 |
| 60 | pictures-fling | completed worker | worker_raster_ms | 10.635 | 13.078 | 14.679 | 15.091 |
| 60 | pictures-fling | completed worker | worker_finish_ms | 0.761 | 0.954 | 1.097 | 1.290 |
| 600 | pictures-cold-ingestion-scroll | serial input | facts_items_ms | 0.032 | 0.040 | 0.076 | 0.077 |
| 600 | pictures-cold-ingestion-scroll | serial input | text_ms | 0.244 | 0.371 | 0.507 | 0.535 |
| 600 | pictures-cold-ingestion-scroll | serial input | tess_ms | 0.115 | 0.272 | 0.330 | 0.344 |
| 600 | pictures-cold-ingestion-scroll | serial input | raster_ms | 35.014 | 37.866 | 39.233 | 40.493 |
| 600 | pictures-cold-ingestion-scroll | serial input | finish_ms | 1.545 | 2.206 | 2.735 | 2.775 |
| 600 | pictures-cold-ingestion-scroll | worker input | facts_items_ms | 0.055 | 0.089 | 0.107 | 0.116 |
| 600 | pictures-cold-ingestion-scroll | worker input | text_ms | 0.072 | 0.134 | 0.168 | 0.184 |
| 600 | pictures-cold-ingestion-scroll | worker input | tess_ms | 0.040 | 0.074 | 0.079 | 0.085 |
| 600 | pictures-cold-ingestion-scroll | worker input | raster_ms | 0.040 | 0.076 | 0.081 | 0.141 |
| 600 | pictures-cold-ingestion-scroll | worker input | finish_ms | 1.695 | 3.019 | 3.260 | 3.724 |
| 600 | pictures-cold-ingestion-scroll | completed worker | worker_items_ms | 0.007 | 0.013 | 0.037 | 0.037 |
| 600 | pictures-cold-ingestion-scroll | completed worker | worker_text_ms | 0.254 | 0.425 | 0.551 | 0.551 |
| 600 | pictures-cold-ingestion-scroll | completed worker | worker_tess_ms | 0.111 | 0.201 | 0.336 | 0.336 |
| 600 | pictures-cold-ingestion-scroll | completed worker | worker_raster_ms | 12.572 | 15.921 | 16.889 | 16.889 |
| 600 | pictures-cold-ingestion-scroll | completed worker | worker_finish_ms | 0.809 | 1.078 | 1.429 | 1.429 |
| 600 | pictures-cold-ingestion-fling | serial input | facts_items_ms | 0.030 | 0.040 | 0.102 | 0.115 |
| 600 | pictures-cold-ingestion-fling | serial input | text_ms | 0.217 | 0.297 | 0.488 | 0.586 |
| 600 | pictures-cold-ingestion-fling | serial input | tess_ms | 0.112 | 0.212 | 0.267 | 0.309 |
| 600 | pictures-cold-ingestion-fling | serial input | raster_ms | 35.175 | 37.029 | 38.817 | 39.080 |
| 600 | pictures-cold-ingestion-fling | serial input | finish_ms | 1.510 | 1.919 | 2.219 | 2.446 |
| 600 | pictures-cold-ingestion-fling | worker input | facts_items_ms | 0.067 | 0.094 | 0.107 | 0.277 |
| 600 | pictures-cold-ingestion-fling | worker input | text_ms | 0.087 | 0.149 | 0.199 | 0.363 |
| 600 | pictures-cold-ingestion-fling | worker input | tess_ms | 0.045 | 0.074 | 0.111 | 0.183 |
| 600 | pictures-cold-ingestion-fling | worker input | raster_ms | 0.041 | 0.078 | 0.103 | 0.219 |
| 600 | pictures-cold-ingestion-fling | worker input | finish_ms | 2.207 | 3.144 | 3.467 | 3.882 |
| 600 | pictures-cold-ingestion-fling | completed worker | worker_items_ms | 0.007 | 0.011 | 0.024 | 0.024 |
| 600 | pictures-cold-ingestion-fling | completed worker | worker_text_ms | 0.245 | 0.417 | 0.564 | 0.712 |
| 600 | pictures-cold-ingestion-fling | completed worker | worker_tess_ms | 0.111 | 0.207 | 0.341 | 0.354 |
| 600 | pictures-cold-ingestion-fling | completed worker | worker_raster_ms | 13.529 | 16.250 | 17.013 | 17.434 |
| 600 | pictures-cold-ingestion-fling | completed worker | worker_finish_ms | 0.845 | 1.180 | 1.387 | 1.819 |
| 600 | pictures-fling | serial input | facts_items_ms | 0.031 | 0.040 | 0.073 | 0.113 |
| 600 | pictures-fling | serial input | text_ms | 0.225 | 0.370 | 0.486 | 0.607 |
| 600 | pictures-fling | serial input | tess_ms | 0.110 | 0.164 | 0.249 | 0.382 |
| 600 | pictures-fling | serial input | raster_ms | 35.441 | 38.095 | 39.866 | 42.093 |
| 600 | pictures-fling | serial input | finish_ms | 1.539 | 2.278 | 2.933 | 3.052 |
| 600 | pictures-fling | worker input | facts_items_ms | 0.053 | 0.081 | 0.098 | 0.169 |
| 600 | pictures-fling | worker input | text_ms | 0.064 | 0.120 | 0.129 | 0.147 |
| 600 | pictures-fling | worker input | tess_ms | 0.035 | 0.051 | 0.092 | 0.101 |
| 600 | pictures-fling | worker input | raster_ms | 0.040 | 0.051 | 0.059 | 0.071 |
| 600 | pictures-fling | worker input | finish_ms | 1.490 | 1.875 | 2.151 | 2.413 |
| 600 | pictures-fling | completed worker | worker_items_ms | 0.005 | 0.012 | 0.018 | 0.025 |
| 600 | pictures-fling | completed worker | worker_text_ms | 0.235 | 0.321 | 0.395 | 0.438 |
| 600 | pictures-fling | completed worker | worker_tess_ms | 0.100 | 0.124 | 0.207 | 0.257 |
| 600 | pictures-fling | completed worker | worker_raster_ms | 10.497 | 12.294 | 14.299 | 14.547 |
| 600 | pictures-fling | completed worker | worker_finish_ms | 0.733 | 0.952 | 0.997 | 1.044 |
| 6000 | pictures-cold-ingestion-scroll | serial input | facts_items_ms | 0.037 | 0.045 | 0.061 | 0.068 |
| 6000 | pictures-cold-ingestion-scroll | serial input | text_ms | 0.237 | 0.279 | 0.427 | 0.535 |
| 6000 | pictures-cold-ingestion-scroll | serial input | tess_ms | 0.111 | 0.196 | 0.220 | 0.331 |
| 6000 | pictures-cold-ingestion-scroll | serial input | raster_ms | 34.044 | 35.975 | 36.399 | 36.748 |
| 6000 | pictures-cold-ingestion-scroll | serial input | finish_ms | 1.305 | 1.595 | 2.165 | 2.429 |
| 6000 | pictures-cold-ingestion-scroll | worker input | facts_items_ms | 0.057 | 0.124 | 0.152 | 0.162 |
| 6000 | pictures-cold-ingestion-scroll | worker input | text_ms | 0.065 | 0.121 | 0.136 | 0.192 |
| 6000 | pictures-cold-ingestion-scroll | worker input | tess_ms | 0.037 | 0.059 | 0.081 | 0.145 |
| 6000 | pictures-cold-ingestion-scroll | worker input | raster_ms | 0.040 | 0.053 | 0.062 | 0.065 |
| 6000 | pictures-cold-ingestion-scroll | worker input | finish_ms | 1.553 | 1.781 | 1.837 | 1.847 |
| 6000 | pictures-cold-ingestion-scroll | completed worker | worker_items_ms | 0.008 | 0.021 | 0.048 | 0.048 |
| 6000 | pictures-cold-ingestion-scroll | completed worker | worker_text_ms | 0.249 | 0.395 | 0.519 | 0.519 |
| 6000 | pictures-cold-ingestion-scroll | completed worker | worker_tess_ms | 0.106 | 0.214 | 0.316 | 0.316 |
| 6000 | pictures-cold-ingestion-scroll | completed worker | worker_raster_ms | 10.568 | 12.357 | 14.485 | 14.485 |
| 6000 | pictures-cold-ingestion-scroll | completed worker | worker_finish_ms | 0.772 | 0.949 | 1.078 | 1.078 |
| 6000 | pictures-cold-ingestion-fling | serial input | facts_items_ms | 0.037 | 0.050 | 0.101 | 0.109 |
| 6000 | pictures-cold-ingestion-fling | serial input | text_ms | 0.223 | 0.302 | 0.591 | 0.600 |
| 6000 | pictures-cold-ingestion-fling | serial input | tess_ms | 0.110 | 0.169 | 0.251 | 0.266 |
| 6000 | pictures-cold-ingestion-fling | serial input | raster_ms | 34.875 | 36.881 | 38.125 | 38.433 |
| 6000 | pictures-cold-ingestion-fling | serial input | finish_ms | 1.395 | 1.748 | 2.302 | 2.312 |
| 6000 | pictures-cold-ingestion-fling | worker input | facts_items_ms | 0.059 | 0.123 | 0.139 | 0.141 |
| 6000 | pictures-cold-ingestion-fling | worker input | text_ms | 0.065 | 0.124 | 0.175 | 0.382 |
| 6000 | pictures-cold-ingestion-fling | worker input | tess_ms | 0.038 | 0.055 | 0.078 | 0.091 |
| 6000 | pictures-cold-ingestion-fling | worker input | raster_ms | 0.040 | 0.053 | 0.061 | 0.111 |
| 6000 | pictures-cold-ingestion-fling | worker input | finish_ms | 1.599 | 1.859 | 2.082 | 2.619 |
| 6000 | pictures-cold-ingestion-fling | completed worker | worker_items_ms | 0.008 | 0.016 | 0.025 | 0.037 |
| 6000 | pictures-cold-ingestion-fling | completed worker | worker_text_ms | 0.235 | 0.348 | 0.445 | 0.598 |
| 6000 | pictures-cold-ingestion-fling | completed worker | worker_tess_ms | 0.105 | 0.199 | 0.274 | 0.286 |
| 6000 | pictures-cold-ingestion-fling | completed worker | worker_raster_ms | 10.674 | 12.428 | 13.400 | 14.036 |
| 6000 | pictures-cold-ingestion-fling | completed worker | worker_finish_ms | 0.786 | 0.972 | 1.192 | 1.534 |
| 6000 | pictures-fling | serial input | facts_items_ms | 0.037 | 0.046 | 0.354 | 2.779 |
| 6000 | pictures-fling | serial input | text_ms | 0.214 | 0.347 | 0.532 | 0.551 |
| 6000 | pictures-fling | serial input | tess_ms | 0.104 | 0.143 | 0.161 | 0.194 |
| 6000 | pictures-fling | serial input | raster_ms | 34.370 | 35.819 | 37.651 | 37.708 |
| 6000 | pictures-fling | serial input | finish_ms | 1.310 | 1.605 | 2.174 | 2.644 |
| 6000 | pictures-fling | worker input | facts_items_ms | 0.060 | 0.121 | 0.145 | 0.417 |
| 6000 | pictures-fling | worker input | text_ms | 0.065 | 0.110 | 0.137 | 0.398 |
| 6000 | pictures-fling | worker input | tess_ms | 0.038 | 0.056 | 0.070 | 0.134 |
| 6000 | pictures-fling | worker input | raster_ms | 0.040 | 0.053 | 0.062 | 0.066 |
| 6000 | pictures-fling | worker input | finish_ms | 1.640 | 1.957 | 2.511 | 3.007 |
| 6000 | pictures-fling | completed worker | worker_items_ms | 0.008 | 0.013 | 0.041 | 0.045 |
| 6000 | pictures-fling | completed worker | worker_text_ms | 0.236 | 0.307 | 0.379 | 0.601 |
| 6000 | pictures-fling | completed worker | worker_tess_ms | 0.103 | 0.131 | 0.177 | 0.417 |
| 6000 | pictures-fling | completed worker | worker_raster_ms | 10.698 | 11.861 | 13.009 | 13.162 |
| 6000 | pictures-fling | completed worker | worker_finish_ms | 0.825 | 0.994 | 1.227 | 2.360 |

## Original baseline, before implementation (1760x990)

| Entries | Motion | Phase | p50 ms | p95 | p99 | max |
|---:|---|---|---:|---:|---:|---:|
| 60 | qm-sheet-pictures-scroll | turn | 54.721 | 58.099 | 58.951 | 60.614 |
| 60 | qm-sheet-pictures-scroll | text/layout | 0.235 | 0.357 | 0.563 | 0.632 |
| 60 | qm-sheet-pictures-scroll | textures/tessellation | 0.106 | 0.212 | 0.282 | 0.387 |
| 60 | qm-sheet-pictures-scroll | raster | 53.508 | 56.775 | 57.957 | 59.238 |
| 60 | qm-sheet-pictures-scroll | finish/copy | 0.635 | 0.875 | 0.889 | 1.431 |
| 60 | qm-sheet-pictures-scroll | facts/items/other | 0.033 | 0.073 | 0.200 | 0.232 |
| 60 | pictures-fling | turn | 54.115 | 60.867 | 78.783 | 79.184 |
| 60 | pictures-fling | text/layout | 0.222 | 0.351 | 0.510 | 0.641 |
| 60 | pictures-fling | textures/tessellation | 0.108 | 0.180 | 0.231 | 0.342 |
| 60 | pictures-fling | raster | 52.973 | 59.547 | 77.494 | 77.787 |
| 60 | pictures-fling | finish/copy | 0.649 | 0.892 | 1.420 | 3.584 |
| 60 | pictures-fling | facts/items/other | 0.034 | 0.066 | 0.217 | 0.255 |
| 600 | qm-sheet-pictures-scroll | turn | 56.538 | 67.850 | 69.190 | 71.368 |
| 600 | qm-sheet-pictures-scroll | text/layout | 0.252 | 0.435 | 0.694 | 0.716 |
| 600 | qm-sheet-pictures-scroll | textures/tessellation | 0.119 | 0.229 | 0.313 | 0.319 |
| 600 | qm-sheet-pictures-scroll | raster | 55.060 | 65.875 | 67.038 | 69.220 |
| 600 | qm-sheet-pictures-scroll | finish/copy | 0.749 | 1.318 | 1.463 | 1.689 |
| 600 | qm-sheet-pictures-scroll | facts/items/other | 0.040 | 0.081 | 0.242 | 0.262 |
| 600 | pictures-fling | turn | 55.179 | 66.002 | 77.763 | 102.933 |
| 600 | pictures-fling | text/layout | 0.244 | 0.429 | 0.595 | 0.847 |
| 600 | pictures-fling | textures/tessellation | 0.115 | 0.225 | 0.268 | 0.311 |
| 600 | pictures-fling | raster | 53.990 | 64.498 | 76.013 | 101.048 |
| 600 | pictures-fling | finish/copy | 0.679 | 1.018 | 1.290 | 1.290 |
| 600 | pictures-fling | facts/items/other | 0.038 | 0.071 | 0.258 | 0.396 |
| 6000 | qm-sheet-pictures-scroll | turn | 54.514 | 89.882 | 110.862 | 147.924 |
| 6000 | qm-sheet-pictures-scroll | text/layout | 0.245 | 0.464 | 0.619 | 0.992 |
| 6000 | qm-sheet-pictures-scroll | textures/tessellation | 0.114 | 0.225 | 0.268 | 0.663 |
| 6000 | qm-sheet-pictures-scroll | raster | 53.214 | 87.772 | 109.018 | 145.861 |
| 6000 | qm-sheet-pictures-scroll | finish/copy | 0.711 | 1.391 | 1.589 | 30.541 |
| 6000 | qm-sheet-pictures-scroll | facts/items/other | 0.050 | 0.095 | 0.269 | 0.310 |
| 6000 | pictures-fling | turn | 53.743 | 57.265 | 60.307 | 60.480 |
| 6000 | pictures-fling | text/layout | 0.217 | 0.305 | 0.573 | 0.664 |
| 6000 | pictures-fling | textures/tessellation | 0.109 | 0.195 | 0.249 | 0.252 |
| 6000 | pictures-fling | raster | 52.507 | 55.703 | 58.510 | 59.140 |
| 6000 | pictures-fling | finish/copy | 0.677 | 0.822 | 1.134 | 1.362 |
| 6000 | pictures-fling | facts/items/other | 0.048 | 0.077 | 0.251 | 0.270 |

## Implementation and merge

- Ordinary native pages paint on one CPU worker. There is one active render, one replaceable request, one replaceable result; epochs and gallery identity reject obsolete pages, including A-B-A. Pixel buffers, hit items and the Facts snapshot travel together. Input polling never joins this worker or waits for its rasterization.
- Independent software-raster rows preserve triangle order and arithmetic. Only the worker opts into four row bands for sufficiently large damage. A serial/parallel byte-equality test covers textures, alpha, text, movement and partial damage.
- The last valid ordinary-page image and its hit geometry are restored on revisits, then refreshed. Page pixels retained in the cache are capped at 96 MiB. Full output damage is used when publishing a worker result; skipped results cannot leave stale strips.
- Recents no longer forces a filesystem relist merely because Pictures is selected again. Decoder completion has a four-entry bounded channel and ingestion consumes at most two completions per turn. Dark-thumbnail lifting runs on decoder workers. Existing visible-range thumbnail eviction stays in place. Stable source paths preserve the first visible item and fractional scroll offset during insertions, and remap held/pending/opened picture actions as indices change.
- Unchanged Results snapshots consume MenuHost's dirty flag rather than painting forever. The currently visible world-video texture is reused and obsolete video handles are evicted instead of cloning/recreating that texture on every refresh.
- The measured main-thread composition cost led to a direct copy for fully opaque page pixels; transparent/translucent corners and holes retain the scalar premultiplied blend. A regression compares all 256 alpha values byte for byte. The final harness counters include Facts generation, item layout, main-thread publication copies, shaped-part finishing and CPU composition.
- Language/look invalidation advances the epoch and empties the page cache. Old-style frames cannot be accepted afterward.

Merged local main d17dabc9d15c0c50ae6415ec5442f9e301bddb22 in merge commit 7b201c8d3005c4dee62f1fc728f443225d777128. Conflicts were appended flow tests and diagnostic exclusions; both sides were retained. The shell's 1408x792 page, layout, hidden wing, toggle-to-close behavior and single composed sheet are preserved. Follow-up integration applies the shell corner mask inside the worker, renders the header from the displayed page's Facts, clears a cold page's old pixels/targets, and seeds a cached page when its back first appears. SteamVR receives one composed sheet submission after all part updates, with union damage covering header and translated page changes. The benchmark uses this composition too. After the wing closes, tools absent from it are reached through the real All tools page, including Settings. Its navigation closes the sheet through the real Close control when the hidden wing makes a tool unavailable; direct rapid-switch fixtures explicitly select matching MenuHost States/Instances/Results state.

MenuHost/Worlds now consume the outer page dirty request after drawing; their own dirty/animation state continues to govern repainting. The regression fails when this bit remains stuck. This removes false non-settling harness states without relaxing rest or performance thresholds.

Cross-stream changes are confined to the Stage/Canvas pipeline, the minimal SheetPicture/OrbitVr publication adapter in vr.rs, shell/worker integration tests, a Worlds thumbnail-source accessor, Recents, the software rasterizer and the harness. No shell geometry/layout redesign was added.

## Limits and decisions

The chosen strategy is a latest-only CPU page worker, plus independent raster rows. First visits can show a title and empty body until a complete frame arrives; the blank body has no hit targets. Immediate last-picture restoration applies to visited ordinary pages. Moving retained image strips and rendering exposed strips was not added: rasterization dominated baseline cost, and moving that work off the input path addressed the largest measured cause without adding a second scrolling renderer.

MenuHost's own Menu page and the Worlds module still paint synchronously. Their live draw/input state is owned by those modules; simply sending it to the worker would decouple their hit handling from the displayed picture. They remain an explicit unresolved part of the requirement that other input-thread work must never wait for any page. The report does not call the all-page 11 ms target passed. Broader conversion of those modules needs a captured draw/input snapshot through the same worker before that guarantee can be made.

The harness does not exercise actual controllers, walking, poses, driver upload or headset delivery. No physical headset acceptance or GPU submission performance is claimed. Frames retain full page damage, so upload bandwidth may rise. Memory bounds are structural (pending/result slots, page-pixel cap, decoder channel and visible-thumbnail retention), not a measured RSS ceiling; gallery metadata scales with the number of entries. Long-duration memory churn and Mac execution were not tested.

No F11/F12 code-path claim in the supplied research was disproved. Research F11/F12 correctly identified synchronous rasterization, unconditional reopen invalidation/relisting, full-page scrolling and progressive ingestion. The baseline confirms raster, rather than text, as the dominant CPU cost. Input-loop isolation is implemented for ordinary pages; treating the same change as automatically covering MenuHost/Worlds would be incorrect. The shell's former 1760x990 dimensions in the research are superseded by the merged shell stream.

All work is committed locally for the orchestrator. No push, bump, release or deployment was requested or performed. This is an implementation/evidence handoff, not a build for Enzo to try from a scratch folder.

Original-baseline qualification: its separately controlled cold matrix was missing, and its early all-page switch fixtures did not consistently set the matching MenuHost state. Those switch cells are not reused as final evidence. A 600-entry original run began near the end of the 60-entry run, so their tails may include overlap. Final corrected serial/worker fixtures use the same merged code, resolution and script, and run sequentially. Earlier exploratory 1760x990 repeats varied substantially (a corrected 600-entry fixture fell as low as 14.07 frames/s); those are retained in their original folders and are not substituted for the final merged measurements.

## Validation commands and results

All Cargo work ran through `C:/Users/Enzo/.orbiters-build/sota/bench/deskcargo.sh <this-worktree> ...`, with the shared target, CARGO_INCREMENTAL=0, and sources frozen throughout the queued/running batch. Final logs are `paint-merged-final-*.log`.

```text
test -p orbiters-desktop --config profile.test.opt-level=2 overlay::orbit -- --test-threads=1
test -p orbiters-desktop --config profile.test.opt-level=2 overlay::worlds -- --test-threads=1
test -p orbiters-ui --config profile.test.opt-level=2 software -- --test-threads=1
test -p orbiters-i18n
build -p orbiters-desktop --config profile.dev.opt-level=2
```

- orbit: test result: ok. 181 passed; 0 failed; 2 ignored; 0 measured; 688 filtered out; finished in 46.58s; EXIT=0.
- worlds: test result: ok. 62 passed; 0 failed; 2 ignored; 0 measured; 807 filtered out; finished in 0.80s; EXIT=0.
- ui: test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 98 filtered out; finished in 0.23s; EXIT=0.
- i18n: test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s; test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.57s; test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s; EXIT=0.
- build:     Finished `dev` profile [optimized + debuginfo] target(s) in 2m 52s; EXIT=0.

Regression coverage includes bounded latest-request replacement, rejecting old-page/old-epoch/A-B-A and reordered-gallery results, publishing pixel/item pairs, exact serial/parallel software-raster output, worker/serial native-page comparison, source-preserving click remapping, fractional gallery anchoring, Results idle behavior, texture reuse, consumed native dirty flags, worker and serial modern/retro corner masks, header/page snapshot agreement, cached next-turn restoration, style invalidation, and composed damage union. The new scalar-compositor regression covers all 256 alpha values. These tests do not establish complete replay hash parity.

Benchmark command for each entry count N, serial and worker sequentially from the same copied optimized executable:

```text
paint-merged.exe --orbit-paint-bench --only vr --runs 1 --entries N --out paint-merged-serial-N --sync
paint-merged.exe --orbit-paint-bench --only vr --runs 1 --entries N --out paint-merged-worker-N
```

No `--upload` was used. The executable SHA-256 is recorded in `paint-merged-build-sha256.txt`. Raw CSVs, full phase percentile tables, captured picture logs and composed PNGs remain in each case folder. The first merged 60-entry checkpoint is preserved as `paint-compose-before-*`; its main-thread finish/composition dominated the remaining ordinary-page cost, selecting the opaque-copy fast path. An interrupted 600-entry checkpoint is named `paint-compose-before-serial-600-cancelled` and is not used for final results. Launcher failures were external harness-process tracking issues, corrected before the final matrix.

## Exact frame comparison

| Entries | Raw parts identical/common | PNGs identical/common | Maximum differing channel level |
|---:|---:|---:|---:|
| 60 | 234/261 | 174/225 | 180 |
| 600 | 234/261 | 174/225 | 3 |
| 6000 | 234/261 | 174/225 | 3 |

The final largest 60-entry Worlds difference is a visible thumbnail in serial versus a placeholder in worker (55,276 pixels, up to 180 channel levels in the composed image). Worlds uses synchronous drawing in both modes. The source arrival differed between captures, so that state is not controlled-data settled-image proof. At 600/6000 the largest composed difference is 2,682/2,685 pixels, up to three levels. These are reported, not discarded.

The full exact difference lists are `paint-merged-raw-frame-comparison.json` and `paint-merged-frame-comparison.json`; composed sheet images are included. In the earlier merged checkpoint, differences were concentrated at text edges: Pictures header differed in five pixels by one level, Instances page in 273 pixels by up to two levels. A same-mode serial repeat differed only on the dynamic Worlds page. Font-atlas/layout history is a plausible cause for the other tiny differences, consistent with Canvas::renew's existing documented behavior, but it has not been proven or repaired. Exact parity remains a blocker; visually similar is not a pass.

## Memory observations

These are maximum sampled private bytes and OS peak working set observations over the full scripted process, not a constant-memory proof. Larger galleries retain more metadata. The channel/mailbox and page-cache limits are structural and tested where noted above; long-duration churn remains unverified.

| Case | Peak working set MiB | Maximum sampled private MiB | Exit |
|---|---:|---:|---:|
| paint-merged-serial-60 | 129.2 | 109.7 | 0 |
| paint-merged-worker-60 | 175.9 | 162.1 | 0 |
| paint-merged-serial-600 | 130.5 | 117.2 | 0 |
| paint-merged-worker-600 | 176.5 | 152.7 | 0 |
| paint-merged-serial-6000 | 135.8 | 121.4 | 0 |
| paint-merged-worker-6000 | 182.3 | 160.2 | 0 |

## Harness notes

- serial 60: no missing-action/non-settling notes.
- worker 60: no missing-action/non-settling notes.
- serial 600: no missing-action/non-settling notes.
- worker 600: no missing-action/non-settling notes.
- serial 6000: no missing-action/non-settling notes.
- worker 6000: no missing-action/non-settling notes.

## Visual checks and unverified work

The initial pre-merge static screenshot comparison had 265/265 PNG files byte-identical (53 scenes in en/fr/ja/es/pt-BR); all five Pictures renders were viewed at original size. This is serial screenshot-path evidence and does not replace worker replay comparison. Final merged CPU screenshots are under `shots-paint/merged-<language>`: all five commands exited 0, with 53 PNGs each (265 total). The five Pictures screenshots, actual composed gallery, the post-switch fallback page, and both 60-entry Worlds counterparts were viewed at original resolution. See `paint-merged-visual-audit.md` for inspected paths and observations. No new Pictures clipping/overlap was observed; this does not certify headset appearance. The post-script normal Instances action reaches a mock friend fallback with States retained; it is not a validated Instances API workflow. The directly selected rapid-switch fixtures were checked separately and have no requested/actual Sheet mismatches in either mode at any size.

Still unverified: physical headset comfort and latency, actual controller/walk/pose loop latency, SteamVR/D3D upload behavior and bandwidth, Mac runtime, and sustained memory churn. Remaining implementation gates: synchronous MenuHost/Worlds painting, any missed 11 ms / 45 fps measurements shown above, and exact replay hash parity. Do not describe this branch as satisfying all F11/F12 requirements or as ready for unqualified release.
