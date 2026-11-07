# Translation batches

4040 different texts, 23845 words to translate into 4 languages. 124 of them (204 words) are in `catalog/website` already, from the website, word for word. 269 texts are used by more than one batch: see "The same text in two batches" in BATCH-BRIEF.md.

Each file is in one batch, so batches can be wrapped side by side and merge cleanly. Strings and words are of what is not yet wrapped, errors apart: "Errors" are error strings, also to translate (Enzo) and held to the test.

| Batch | What | Files | Strings | Words | Not in `website` | Errors |
|---|---|---:|---:|---:|---:|---:|
| `orbit` | The menus around VRChat's: wheel, wing, strip, All tools and their sheets | 4 | 319 | 1350 | 224 | 1 |
| `overlay-worlds` | Worlds in 3D: its side panel, pages and hints | 12 | 296 | 1076 | 234 | 42 |
| `overlay-rest` | The other overlays: VR panel, N4N0's menu pages, its keyboard | 11 | 211 | 911 | 182 | 12 |
| `n4n0` | N4N0 itself: what its menu writes live, the assistant, its messages | 32 | 448 | 2426 | 401 | 64 |
| `pages-settings` | App window: Settings and what it opens | 10 | 619 | 4091 | 556 | 0 |
| `pages-capture` | App window: Home, Capture, Sessions, reconstruction | 9 | 506 | 2696 | 408 | 6 |
| `pages-gallery` | App window: Gallery, Sharing, Community, Events | 10 | 487 | 2144 | 351 | 9 |
| `pages-editor` | App window: the photo and 3D editor | 8 | 214 | 839 | 181 | 5 |
| `pages-projects` | App window: Projects | 46 | 884 | 4423 | 709 | 198 |
| `pages-worlds` | App window: Worlds, Videos, Saved looks and the rest of the pages | 3 | 188 | 847 | 158 | 0 |
| `controller` | What the service reports to the windows: status, toasts, the tray | 57 | 495 | 3462 | 459 | 192 |
| `foundation-crates` | Setup, shared widgets and the lower crates | 79 | 458 | 2145 | 369 | 381 |

## `orbit`: The menus around VRChat's: wheel, wing, strip, All tools and their sheets

- `crates/orbiters-desktop/src/overlay/orbit/panels.rs`: 258 strings, 1106 words
- `crates/orbiters-desktop/src/overlay/orbit/mod.rs`: 48 strings, 163 words
- `crates/orbiters-desktop/src/overlay/orbit/desktop.rs`: 12 strings, 63 words, 1 errors
- `crates/orbiters-desktop/src/overlay/orbit/vr.rs`: 1 strings, 18 words

## `overlay-worlds`: Worlds in 3D: its side panel, pages and hints

- `crates/orbiters-desktop/src/overlay/worlds/panel.rs`: 128 strings, 441 words
- `crates/orbiters-desktop/src/overlay/worlds/panel/shots.rs`: 57 strings, 210 words
- `crates/orbiters-desktop/src/overlay/worlds/panel/pages.rs`: 38 strings, 146 words
- `crates/orbiters-desktop/src/overlay/worlds/mod.rs`: 23 strings, 137 words, 5 errors
- `crates/orbiters-desktop/src/overlay/worlds/small_hint.rs`: 15 strings, 53 words, 4 errors
- `crates/orbiters-desktop/src/overlay/worlds/view.rs`: 7 strings, 32 words
- `crates/orbiters-desktop/src/overlay/worlds/render.rs`: 11 strings, 27 words, 6 errors
- `crates/orbiters-desktop/src/overlay/worlds/panel/bar.rs`: 9 strings, 17 words
- `crates/orbiters-desktop/src/overlay/worlds/finder.rs`: 7 strings, 10 words, 3 errors
- `crates/orbiters-desktop/src/overlay/worlds/interop.rs`: 1 strings, 3 words, 18 errors
- `crates/orbiters-desktop/src/overlay/worlds/catalog.rs`: 0 strings, 0 words, 4 errors
- `crates/orbiters-desktop/src/overlay/worlds/shots.rs`: 0 strings, 0 words, 2 errors

## `overlay-rest`: The other overlays: VR panel, N4N0's menu pages, its keyboard

- `crates/orbiters-desktop/src/overlay/ui.rs`: 85 strings, 398 words
- `crates/orbiters-desktop/src/overlay/menu_pages.rs`: 79 strings, 340 words, 3 errors
- `crates/orbiters-desktop/src/overlay/calibration.rs`: 23 strings, 91 words
- `crates/orbiters-desktop/src/overlay/actions.rs`: 5 strings, 30 words
- `crates/orbiters-desktop/src/overlay/coverage.rs`: 6 strings, 16 words, 1 errors
- `crates/orbiters-desktop/src/overlay/mod.rs`: 5 strings, 13 words, 3 errors
- `crates/orbiters-desktop/src/overlay/floating/laser.rs`: 3 strings, 11 words, 1 errors
- `crates/orbiters-desktop/src/overlay/floating/panel.rs`: 3 strings, 8 words, 1 errors
- `crates/orbiters-desktop/src/overlay/floating/watch.rs`: 1 strings, 2 words, 1 errors
- `crates/orbiters-desktop/src/overlay/menu.rs`: 1 strings, 2 words, 1 errors
- `crates/orbiters-desktop/src/overlay/eye_picture.rs`: 0 strings, 0 words, 1 errors

## `n4n0`: N4N0 itself: what its menu writes live, the assistant, its messages

- `crates/orbiters-n4n0-core/src/menu/layout.rs`: 141 strings, 522 words
- `crates/orbiters-n4n0/src/runtime.rs`: 51 strings, 327 words, 7 errors
- `crates/orbiters-n4n0/src/screen.rs`: 36 strings, 264 words, 1 errors
- `crates/orbiters-n4n0-core/src/policy.rs`: 34 strings, 257 words
- `crates/orbiters-n4n0-core/src/menu/mod.rs`: 46 strings, 198 words
- `crates/orbiters-n4n0-core/src/context.rs`: 11 strings, 129 words
- `crates/orbiters-n4n0/src/states.rs`: 12 strings, 115 words, 6 errors
- `crates/orbiters-n4n0-core/src/menu/theme.rs`: 10 strings, 67 words, 7 errors
- `crates/orbiters-n4n0/src/pointer_log.rs`: 6 strings, 60 words
- `crates/orbiters-n4n0-core/src/mirror.rs`: 8 strings, 55 words
- `crates/orbiters-n4n0-core/src/captures.rs`: 15 strings, 53 words
- `crates/orbiters-n4n0/src/tests.rs`: 9 strings, 50 words, 1 errors
- `crates/orbiters-n4n0/src/voice.rs`: 7 strings, 48 words
- `crates/orbiters-n4n0-core/src/states.rs`: 6 strings, 38 words
- `crates/orbiters-n4n0-core/src/feedback.rs`: 10 strings, 34 words
- `crates/orbiters-n4n0/src/status.rs`: 4 strings, 32 words
- `crates/orbiters-n4n0-core/src/qr.rs`: 3 strings, 32 words, 7 errors
- `crates/orbiters-n4n0/src/qr.rs`: 3 strings, 31 words
- `crates/orbiters-n4n0-core/src/assistant.rs`: 3 strings, 26 words, 6 errors
- `crates/orbiters-n4n0-core/src/controls.rs`: 11 strings, 23 words, 6 errors
- `crates/orbiters-n4n0-core/src/invites.rs`: 3 strings, 19 words
- `crates/orbiters-n4n0-core/src/tools.rs`: 3 strings, 12 words, 2 errors
- `crates/orbiters-n4n0-core/src/menu/touch.rs`: 3 strings, 10 words
- `crates/orbiters-n4n0/src/media.rs`: 7 strings, 9 words
- `crates/orbiters-n4n0/src/menu_host.rs`: 1 strings, 6 words
- `crates/orbiters-n4n0/src/platform.rs`: 1 strings, 4 words, 5 errors
- `crates/orbiters-n4n0-core/src/text.rs`: 3 strings, 3 words
- `crates/orbiters-n4n0-core/src/youtube.rs`: 1 strings, 2 words, 3 errors
- `crates/orbiters-n4n0-core/src/drops.rs`: 0 strings, 0 words, 2 errors
- `crates/orbiters-n4n0-core/src/picture.rs`: 0 strings, 0 words, 8 errors
- `crates/orbiters-n4n0-core/src/suggestions.rs`: 0 strings, 0 words, 2 errors
- `crates/orbiters-n4n0-core/src/videos.rs`: 0 strings, 0 words, 1 errors

## `pages-settings`: App window: Settings and what it opens

- `crates/orbiters-desktop/src/pages/settings.rs`: 223 strings, 1662 words
- `crates/orbiters-desktop/src/pages/n4n0.rs`: 189 strings, 1279 words
- `crates/orbiters-desktop/src/pages/experimental.rs`: 43 strings, 311 words
- `crates/orbiters-desktop/src/pages/storage.rs`: 56 strings, 281 words
- `crates/orbiters-desktop/src/pages/network.rs`: 38 strings, 236 words
- `crates/orbiters-desktop/src/pages/vr_audio.rs`: 19 strings, 143 words
- `crates/orbiters-desktop/src/pages/requests.rs`: 31 strings, 130 words
- `crates/orbiters-desktop/src/pages/mic.rs`: 7 strings, 36 words
- `crates/orbiters-desktop/src/pages/mod.rs`: 12 strings, 12 words
- `crates/orbiters-desktop/src/pages/form.rs`: 1 strings, 1 words

## `pages-capture`: App window: Home, Capture, Sessions, reconstruction

- `crates/orbiters-desktop/src/pages/capture.rs`: 113 strings, 612 words
- `crates/orbiters-desktop/src/pages/home.rs`: 132 strings, 600 words
- `crates/orbiters-desktop/src/pages/splats/select.rs`: 65 strings, 451 words, 2 errors
- `crates/orbiters-desktop/src/pages/sessions.rs`: 84 strings, 350 words
- `crates/orbiters-desktop/src/pages/splats.rs`: 44 strings, 253 words, 1 errors
- `crates/orbiters-desktop/src/pages/splat_export.rs`: 31 strings, 217 words
- `crates/orbiters-desktop/src/pages/reconstruct_again.rs`: 24 strings, 162 words
- `crates/orbiters-desktop/src/pages/splats/edit3d.rs`: 9 strings, 33 words
- `crates/orbiters-desktop/src/pages/splat_worker.rs`: 4 strings, 18 words, 3 errors

## `pages-gallery`: App window: Gallery, Sharing, Community, Events

- `crates/orbiters-desktop/src/pages/sharing.rs`: 139 strings, 749 words, 1 errors
- `crates/orbiters-desktop/src/pages/gallery.rs`: 123 strings, 549 words, 1 errors
- `crates/orbiters-desktop/src/pages/events.rs`: 126 strings, 469 words
- `crates/orbiters-desktop/src/pages/gallery_select.rs`: 20 strings, 93 words, 6 errors
- `crates/orbiters-desktop/src/pages/gallery_folders.rs`: 13 strings, 76 words
- `crates/orbiters-desktop/src/pages/community_talk.rs`: 15 strings, 70 words
- `crates/orbiters-desktop/src/pages/community.rs`: 24 strings, 40 words, 1 errors
- `crates/orbiters-desktop/src/pages/gallery_assets.rs`: 18 strings, 40 words
- `crates/orbiters-desktop/src/pages/gallery_layout.rs`: 3 strings, 35 words
- `crates/orbiters-desktop/src/pages/gallery_layers.rs`: 6 strings, 23 words

## `pages-editor`: App window: the photo and 3D editor

- `crates/orbiters-desktop/src/pages/editor/controls.rs`: 106 strings, 453 words
- `crates/orbiters-desktop/src/pages/editor/mod.rs`: 27 strings, 124 words
- `crates/orbiters-desktop/src/pages/editor/layers.rs`: 41 strings, 109 words
- `crates/orbiters-desktop/src/pages/editor/crop.rs`: 24 strings, 97 words
- `crates/orbiters-desktop/src/pages/editor/looks.rs`: 9 strings, 37 words, 1 errors
- `crates/orbiters-desktop/src/pages/editor/canvas.rs`: 3 strings, 11 words
- `crates/orbiters-desktop/src/pages/editor/files.rs`: 3 strings, 5 words, 4 errors
- `crates/orbiters-desktop/src/pages/editor/preview.rs`: 1 strings, 3 words

## `pages-projects`: App window: Projects

- `crates/orbiters-desktop/src/pages/projects/storage_card.rs`: 56 strings, 601 words
- `crates/orbiters-desktop/src/pages/projects/preview.rs`: 71 strings, 561 words
- `crates/orbiters-desktop/src/pages/projects/dialogs.rs`: 101 strings, 513 words
- `crates/orbiters-desktop/src/pages/projects/panel.rs`: 74 strings, 304 words
- `crates/orbiters-desktop/src/pages/projects/state.rs`: 72 strings, 295 words
- `crates/orbiters-desktop/src/pages/projects/repo_cards.rs`: 61 strings, 251 words
- `crates/orbiters-desktop/src/pages/projects/world_card.rs`: 40 strings, 238 words
- `crates/orbiters-desktop/src/pages/projects/ui.rs`: 47 strings, 218 words
- `crates/orbiters-desktop/src/pages/projects/duplicate.rs`: 28 strings, 190 words
- `crates/orbiters-desktop/src/pages/projects/packages_card.rs`: 30 strings, 131 words
- `crates/orbiters-desktop/src/pages/projects/menus.rs`: 30 strings, 117 words
- `crates/orbiters-desktop/src/pages/projects/unity_session.rs`: 23 strings, 113 words
- `crates/orbiters-desktop/src/pages/projects/conflict.rs`: 24 strings, 110 words
- `crates/orbiters-desktop/src/pages/projects/missing_card.rs`: 22 strings, 106 words
- `crates/orbiters-projects/src/scene/build.rs`: 29 strings, 99 words, 17 errors
- `crates/orbiters-desktop/src/pages/projects/preview_gpu.rs`: 39 strings, 85 words, 2 errors
- `crates/orbiters-desktop/src/pages/projects/package_fixtures.rs`: 19 strings, 74 words
- `crates/orbiters-desktop/src/pages/projects/toolkit_update.rs`: 5 strings, 63 words
- `crates/orbiters-projects/src/coherent.rs`: 8 strings, 52 words
- `crates/orbiters-projects/src/discovery.rs`: 15 strings, 40 words
- `crates/orbiters-projects/src/scene/serialized.rs`: 23 strings, 34 words, 21 errors
- `crates/orbiters-projects/src/storage.rs`: 5 strings, 33 words, 19 errors
- `crates/orbiters-projects/src/updates.rs`: 2 strings, 23 words, 2 errors
- `crates/orbiters-desktop/src/pages/projects/vrchat_picture.rs`: 5 strings, 22 words, 1 errors
- `crates/orbiters-projects/src/scene/prefab.rs`: 6 strings, 21 words, 12 errors
- `crates/orbiters-projects/src/blender.rs`: 5 strings, 19 words, 2 errors
- `crates/orbiters-projects/src/templates.rs`: 8 strings, 19 words, 21 errors
- `crates/orbiters-desktop/src/pages/projects/blend.rs`: 6 strings, 18 words
- `crates/orbiters-projects/src/scene/model.rs`: 6 strings, 14 words, 5 errors
- `crates/orbiters-projects/src/scene/material.rs`: 7 strings, 12 words
- `crates/orbiters-desktop/src/pages/projects/builds.rs`: 1 strings, 9 words
- `crates/orbiters-projects/src/lib.rs`: 2 strings, 7 words, 14 errors
- `crates/orbiters-projects/src/preview.rs`: 2 strings, 7 words, 19 errors
- `crates/orbiters-projects/src/scene/mod.rs`: 2 strings, 6 words, 1 errors
- `crates/orbiters-projects/src/scene/world.rs`: 2 strings, 6 words, 1 errors
- `crates/orbiters-projects/src/missing.rs`: 1 strings, 5 words, 2 errors
- `crates/orbiters-projects/src/size.rs`: 3 strings, 3 words, 2 errors
- `crates/orbiters-projects/src/avatars.rs`: 2 strings, 2 words
- `crates/orbiters-projects/src/scene/primitives.rs`: 1 strings, 1 words
- `crates/orbiters-projects/src/scene/terrain.rs`: 1 strings, 1 words, 7 errors
- `crates/orbiters-projects/src/editor.rs`: 0 strings, 0 words, 7 errors
- `crates/orbiters-projects/src/scene/artifact.rs`: 0 strings, 0 words, 2 errors
- `crates/orbiters-projects/src/scene/mesh_asset.rs`: 0 strings, 0 words, 6 errors
- `crates/orbiters-projects/src/scene/texture.rs`: 0 strings, 0 words, 19 errors
- `crates/orbiters-projects/src/setup.rs`: 0 strings, 0 words, 5 errors
- `crates/orbiters-projects/src/vpm.rs`: 0 strings, 0 words, 11 errors

## `pages-worlds`: App window: Worlds, Videos, Saved looks and the rest of the pages

- `crates/orbiters-desktop/src/pages/worlds.rs`: 87 strings, 391 words
- `crates/orbiters-desktop/src/pages/states.rs`: 46 strings, 253 words
- `crates/orbiters-desktop/src/pages/videos.rs`: 55 strings, 203 words

## `controller`: What the service reports to the windows: status, toasts, the tray

- `crates/orbiters-desktop/src/controller/shares.rs`: 40 strings, 317 words, 22 errors
- `crates/orbiters-desktop/src/controller.rs`: 33 strings, 307 words, 19 errors
- `crates/orbiters-desktop/src/controller/hide_self.rs`: 14 strings, 291 words
- `crates/orbiters-desktop/src/splat_export.rs`: 27 strings, 256 words, 3 errors
- `crates/orbiters-desktop/src/state.rs`: 14 strings, 192 words
- `crates/orbiters-desktop/src/media.rs`: 9 strings, 187 words, 2 errors
- `crates/orbiters-desktop/src/controller/world_captures.rs`: 19 strings, 161 words, 1 errors
- `crates/orbiters-desktop/src/controller/bridge_game.rs`: 18 strings, 140 words, 6 errors
- `crates/orbiters-desktop/src/bridge/clients.rs`: 22 strings, 131 words, 3 errors
- `crates/orbiters-desktop/src/controller/calls.rs`: 23 strings, 118 words, 3 errors
- `crates/orbiters-desktop/src/bridge/keeper.rs`: 10 strings, 106 words
- `crates/orbiters-desktop/src/controller/burst.rs`: 8 strings, 106 words, 2 errors
- `crates/orbiters-desktop/src/app.rs`: 34 strings, 103 words
- `crates/orbiters-desktop/src/bridge/mcp.rs`: 26 strings, 101 words, 14 errors
- `crates/orbiters-desktop/src/tracking.rs`: 13 strings, 98 words, 1 errors
- `crates/orbiters-desktop/src/bridge/lab.rs`: 19 strings, 63 words, 6 errors
- `crates/orbiters-desktop/src/controller/update.rs`: 8 strings, 62 words
- `crates/orbiters-desktop/src/streams.rs`: 21 strings, 62 words
- `crates/orbiters-desktop/src/bridge/capture.rs`: 14 strings, 60 words, 8 errors
- `crates/orbiters-desktop/src/controller/cleanup.rs`: 14 strings, 58 words, 3 errors
- `crates/orbiters-desktop/src/controller/vr_audio.rs`: 9 strings, 57 words
- `crates/orbiters-desktop/src/controller/maps.rs`: 11 strings, 50 words, 10 errors
- `crates/orbiters-desktop/src/controller/vrchat_data.rs`: 6 strings, 47 words, 17 errors
- `crates/orbiters-desktop/src/bridge/mod.rs`: 7 strings, 36 words, 4 errors
- `crates/orbiters-desktop/src/controller/menu_shares.rs`: 4 strings, 35 words, 5 errors
- `crates/orbiters-desktop/src/bridge/win.rs`: 4 strings, 29 words, 12 errors
- `crates/orbiters-desktop/src/drag_out.rs`: 8 strings, 29 words, 4 errors
- `crates/orbiters-desktop/src/controller/world_feedback.rs`: 3 strings, 25 words, 2 errors
- `crates/orbiters-desktop/src/mac.rs`: 6 strings, 25 words, 2 errors
- `crates/orbiters-desktop/src/controller/account.rs`: 3 strings, 23 words, 3 errors
- `crates/orbiters-desktop/src/mac_update.rs`: 1 strings, 23 words, 9 errors
- `crates/orbiters-desktop/src/controller/around.rs`: 2 strings, 18 words
- `crates/orbiters-desktop/src/controller/scan.rs`: 3 strings, 17 words, 2 errors
- `crates/orbiters-desktop/src/service.rs`: 3 strings, 17 words
- `crates/orbiters-desktop/src/tray.rs`: 5 strings, 14 words, 1 errors
- `crates/orbiters-desktop/src/qr.rs`: 2 strings, 13 words, 1 errors
- `crates/orbiters-desktop/src/backend.rs`: 2 strings, 12 words, 1 errors
- `crates/orbiters-desktop/src/bridge/e2e.rs`: 5 strings, 12 words
- `crates/orbiters-desktop/src/words.rs`: 5 strings, 9 words
- `crates/orbiters-desktop/src/gpu.rs`: 3 strings, 8 words, 1 errors
- `crates/orbiters-desktop/src/device.rs`: 4 strings, 7 words, 1 errors
- `crates/orbiters-desktop/src/bridge/input.rs`: 3 strings, 6 words, 4 errors
- `crates/orbiters-desktop/src/controller/lan.rs`: 2 strings, 6 words
- `crates/orbiters-desktop/src/controller/pause_tests.rs`: 1 strings, 6 words
- `crates/orbiters-desktop/src/startup.rs`: 1 strings, 6 words, 2 errors
- `crates/orbiters-desktop/src/worlds.rs`: 2 strings, 6 words
- `crates/orbiters-desktop/src/controller/n4n0.rs`: 3 strings, 5 words
- `crates/orbiters-desktop/src/coverage_window.rs`: 1 strings, 2 words, 1 errors
- `crates/orbiters-desktop/src/audio_devices.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-desktop/src/controller/vrchat_login.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-desktop/src/ipc.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-desktop/src/main.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-desktop/src/mixer_inbox.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-desktop/src/reconstruct.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-desktop/src/upload_prep.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-desktop/src/views.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-desktop/src/worker.rs`: 0 strings, 0 words, 1 errors

## `foundation-crates`: Setup, shared widgets and the lower crates

- `crates/orbiters-setup/src/ui.rs`: 77 strings, 374 words, 3 errors
- `crates/orbiters-capture/src/session.rs`: 24 strings, 210 words
- `crates/orbiters-core/src/vrchat_api.rs`: 29 strings, 157 words
- `crates/orbiters-sound/src/device.rs`: 18 strings, 142 words, 1 errors
- `crates/orbiters-ui/src/photo/edit.rs`: 29 strings, 113 words
- `crates/orbiters-sound/src/coreaudio.rs`: 15 strings, 88 words, 5 errors
- `crates/orbiters-call/src/main.rs`: 10 strings, 86 words
- `crates/orbiters-capture/src/coverage.rs`: 11 strings, 84 words
- `crates/orbiters-ui/src/fonts.rs`: 33 strings, 59 words
- `crates/orbiters-capture/src/sources.rs`: 11 strings, 58 words, 3 errors
- `crates/orbiters-setup/src/main.rs`: 6 strings, 57 words, 4 errors
- `crates/orbiters-core/src/account.rs`: 8 strings, 55 words, 6 errors
- `crates/orbiters-core/src/picture.rs`: 4 strings, 53 words
- `crates/orbiters-ui/src/photo/gpu.rs`: 19 strings, 50 words, 4 errors
- `crates/orbiters-capture/src/audio.rs`: 9 strings, 49 words, 8 errors
- `crates/orbiters-voice/src/main.rs`: 5 strings, 48 words, 19 errors
- `crates/orbiters-setup/src/install.rs`: 12 strings, 34 words, 15 errors
- `crates/orbiters-capture/src/main.rs`: 9 strings, 33 words, 7 errors
- `crates/orbiters-core/src/osc.rs`: 4 strings, 30 words, 11 errors
- `crates/orbiters-core/src/community.rs`: 4 strings, 28 words, 1 errors
- `crates/orbiters-core/src/vrchat_accounts.rs`: 5 strings, 26 words
- `crates/orbiters-ui/src/blur.rs`: 11 strings, 26 words
- `crates/orbiters-ui/src/format.rs`: 16 strings, 23 words
- `crates/orbiters-core/src/world_feedback.rs`: 5 strings, 22 words
- `crates/orbiters-openvr/src/mirror.rs`: 8 strings, 22 words, 1 errors
- `crates/orbiters-core/src/shares.rs`: 2 strings, 18 words, 2 errors
- `crates/orbiters-openvr/src/applications.rs`: 2 strings, 17 words, 1 errors
- `crates/orbiters-update/src/lib.rs`: 2 strings, 17 words, 12 errors
- `crates/orbiters-capture/src/verify.rs`: 4 strings, 16 words
- `crates/orbiters-setup/src/system.rs`: 2 strings, 15 words, 4 errors
- `crates/orbiters-spout/src/win.rs`: 4 strings, 13 words, 10 errors
- `crates/orbiters-ui/src/glow.rs`: 6 strings, 13 words
- `crates/orbiters-ui/src/offscreen.rs`: 5 strings, 12 words, 4 errors
- `crates/orbiters-openvr/src/gpu.rs`: 4 strings, 11 words, 1 errors
- `crates/orbiters-setup/build.rs`: 2 strings, 11 words
- `crates/orbiters-ui/src/photo/text.rs`: 9 strings, 11 words
- `crates/orbiters-core/src/assets.rs`: 9 strings, 10 words, 2 errors
- `crates/orbiters-audio/src/lib.rs`: 4 strings, 8 words, 6 errors
- `crates/orbiters-call/src/loopback.rs`: 2 strings, 8 words, 2 errors
- `crates/orbiters-core/src/lan.rs`: 2 strings, 6 words, 1 errors
- `crates/orbiters-core/src/settings.rs`: 1 strings, 6 words
- `crates/orbiters-core/src/gallery_talk.rs`: 2 strings, 4 words
- `crates/orbiters-core/src/video_history.rs`: 3 strings, 4 words
- `crates/orbiters-core/src/panic_log.rs`: 1 strings, 3 words
- `crates/orbiters-ui/src/record.rs`: 1 strings, 3 words
- `crates/orbiters-core/src/dpapi.rs`: 1 strings, 2 words, 12 errors
- `crates/orbiters-core/src/vrchat_log.rs`: 1 strings, 2 words
- `crates/orbiters-openvr/src/lib.rs`: 1 strings, 2 words, 1 errors
- `crates/orbiters-ui/src/feedback.rs`: 2 strings, 2 words
- `crates/orbiters-core/src/lib.rs`: 1 strings, 1 words
- `crates/orbiters-core/src/visited.rs`: 1 strings, 1 words
- `crates/orbiters-ui/src/dialog.rs`: 1 strings, 1 words
- `crates/orbiters-ui/src/widgets.rs`: 1 strings, 1 words
- `crates/orbiters-call/src/call.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-capture/src/tracking.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-core/src/bundle.rs`: 0 strings, 0 words, 4 errors
- `crates/orbiters-core/src/cleanup.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-core/src/maps.rs`: 0 strings, 0 words, 9 errors
- `crates/orbiters-core/src/paths.rs`: 0 strings, 0 words, 1 errors
- `crates/orbiters-core/src/vrchat_source.rs`: 0 strings, 0 words, 2 errors
- `crates/orbiters-reconstruct/src/aberration.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-reconstruct/src/checkpoint.rs`: 0 strings, 0 words, 16 errors
- `crates/orbiters-reconstruct/src/dataset.rs`: 0 strings, 0 words, 5 errors
- `crates/orbiters-reconstruct/src/edit.rs`: 0 strings, 0 words, 19 errors
- `crates/orbiters-reconstruct/src/export.rs`: 0 strings, 0 words, 12 errors
- `crates/orbiters-reconstruct/src/gpu.rs`: 0 strings, 0 words, 49 errors
- `crates/orbiters-reconstruct/src/main.rs`: 0 strings, 0 words, 14 errors
- `crates/orbiters-reconstruct/src/mesh/mod.rs`: 0 strings, 0 words, 23 errors
- `crates/orbiters-reconstruct/src/ply.rs`: 0 strings, 0 words, 5 errors
- `crates/orbiters-reconstruct/src/sky.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-reconstruct/src/train.rs`: 0 strings, 0 words, 10 errors
- `crates/orbiters-reconstruct/src/tree_file.rs`: 0 strings, 0 words, 6 errors
- `crates/orbiters-reconstruct/src/tree_tune.rs`: 0 strings, 0 words, 2 errors
- `crates/orbiters-reconstruct/src/web.rs`: 0 strings, 0 words, 3 errors
- `crates/orbiters-smooth/src/control.rs`: 0 strings, 0 words, 5 errors
- `crates/orbiters-smooth/src/install.rs`: 0 strings, 0 words, 17 errors
- `crates/orbiters-update/src/net.rs`: 0 strings, 0 words, 5 errors
- `crates/orbiters-update/src/payload.rs`: 0 strings, 0 words, 14 errors
- `crates/orbiters-update/src/windows_start.rs`: 0 strings, 0 words, 1 errors
