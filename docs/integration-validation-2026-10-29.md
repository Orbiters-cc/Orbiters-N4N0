# Feature integration validation - 29 October 2026

Implemented locally on `main` in the existing checkout. No version bump, tag, push, release, installed-app update,
contracts change or production mutation. The unrelated untracked audit/review files were left untouched.
Routing commit: `d5972d8` - Route VRChat data through the member session.
The following crop commit contains this report and the thumbnail-generation/cache changes.

## Behavior and files

- Unity avatar pictures use Toolkit files first, then the eligible member's `VrchatSource::avatar` and `image`.
  Without a personal session, the existing Orbiters thumbnail endpoint/cache is used. Eight-picture batches publish
  progressively; errors offer the page's Refresh path.
- Share to ... lazily obtains personal friends through the service and merges with VRCX/VRCNext by `usr_` ID,
  preferring direct data. Profile images use `source.image` on workers and never enter the public disk image cache.
  Signed-out members keep local friends/public image loading. Errors have an inline Retry.
- Opening Instance info lazily checks that world/instance. Active/closed results enable/disable Join; unknown,
  private or inaccessible results retain the current-instance behavior. The session client caches results for
  60 seconds (256 entries maximum), checks the gate before cache reads, and clears entries with other session data.
- Read-only requests/replies use correlated, authenticated local IPC. Scope changes clear window data/pending jobs;
  the service rejects requests/results for changed accounts, environments and session revisions. Network calls and
  decoding stay on workers. Image workers stop when their picture collection is discarded.
- New session thumbnails omit top strip/patches and bottom skeleton rows before downscaling. Existing thumbnails get
  a lazy `thumbnail-picture-v2.qoi` cache; both full-frame and already-top-cropped old images are covered without raw
  keyframes. Cleanup preserves/counts this cache. Source frames remain unchanged.

Files, relative to the repository:

| Area | Files |
| --- | --- |
| Core routing | `crates/orbiters-core/src/vrchat_api.rs`, `vrchat_source.rs` |
| Service/IPC | `crates/orbiters-desktop/src/controller/vrchat_data.rs`, `controller.rs`, `backend.rs`, `ipc.rs`, `service.rs` |
| UI/loaders/fixtures | `crates/orbiters-desktop/src/pages/projects/state.rs`, `pages/sharing.rs`, `pages/gallery.rs`, `worlds.rs`, `screenshot.rs` |
| Thumbnail generation/cache | `crates/orbiters-core/src/bundle.rs`, `cleanup.rs`, `crates/orbiters-capture/src/session.rs` |
| Documentation | This report, `docs/vrchat-sign-in.md`, `photos-and-captures.md`, `photos-and-captures-validation-2026-10-29.md`, `unity-projects-validation-2026-10-29.md` |

## Checks

Every Cargo command used `CARGO_TARGET_DIR=C:/Users/Enzo/.orbiters-target` and `CARGO_INCREMENTAL=0`.
No repository target folder was built into or deleted. Formatting used targeted rustfmt with the repository's
configuration and `skip_children=true`; no workspace-wide formatting was run.

| Command | Result |
| --- | --- |
| `cargo test -p orbiters-projects` | 12 passed: 11 contract tests and one disposable VPM lifecycle test. |
| `cargo test -p orbiters-core --lib` | 129 passed, 6 existing live/machine-specific tests ignored. |
| `cargo test -p orbiters-desktop shares` | 4 passed, 2 existing live/machine-specific tests ignored. |
| `cargo test -p orbiters-desktop projects` | 1 passed: avatar aspect ratio, letterboxing and invalid bytes. |
| `cargo test -p orbiters-desktop vrchat_login` | 4 passed. |
| `cargo test -p orbiters-desktop vrchat_data` | 4 passed: scope isolation, disabled instance fallback, avatar decoding, out-of-order IPC correlation and pending cleanup. |
| `cargo test -p orbiters-desktop pages::gallery` | 2 passed: grouping and Join's true/false/unknown behavior. |
| `cargo test -p orbiters-desktop ipc::tests` | 4 passed. |
| `cargo test -p orbiters-capture encoded_picture_thumbnail` | 1 passed: real encoder, BGRA order and cropped cache pixels. |
| `cargo check -p orbiters-desktop` / `cargo build -p orbiters-desktop` | Passed; existing unused upload-method warning remains. |
| `git diff --check` / staged diff checks | Passed. |

Core tests cover exact crop rectangles at 720p/1080p/2160p and tiny-image fallback; both old thumbnail shapes;
cache reuse without double cropping; cleanup preservation/accounting; personal avatar/image/instance routing with
mock cookies; friend preference/deduplication; instance cache expiry/revocation; expired-session fallback without
VRChat cookies reaching Orbiters. The first core run exposed the new display cache in cleanup's size/file
expectations; assertions were updated and the suite then passed.

Logs: `C:/Users/Enzo/.orbiters-integration-qa/*-tests.log` and `desktop-build.log`.

## Screenshots

The external-target executable was run offscreen, selecting each state:

```powershell
orbiters-desktop --screenshot C:/Users/Enzo/.orbiters-integration-qa/after `
  --sessions C:/Users/Enzo/Documents/Orbiters/Sessions --only <state>
```

Every final PNG was opened and visually inspected against references under
`C:/Users/Enzo/.orbiters-sharing-qa/` and `C:/Users/Enzo/.orbiters-project-review/final/`.

Before/after crop pairs share the following names under
`C:/Users/Enzo/.orbiters-integration-qa/before/` and `C:/Users/Enzo/.orbiters-integration-qa/after/`:

- `gallery-mixed.png`: no bottom noise band on capture cards.
- `gallery-owner.png`: modal picture omits the band, preserving cover cropping.
- `worlds.png`: Visited worlds session pictures omit the band. Public metadata can differ because existing
  loaders fetched it between renders.

The `after/` directory also contains inspected `gallery-friends.png`, `gallery-friends-error.png`,
`gallery-instance.png`, `gallery-instance-live.png`, `gallery-instance-closed.png`, `gallery-instance-error.png`,
`world-share-detail.png`, `projects-gallery.png` and `projects-avatars.png`: 12 final standard states total.
Projects gallery uses height 1000 so both proposal Add buttons are visible; the other standard states are 1280 by 820.
`narrow/` contains inspected 920-by-640 `gallery-friends.png`, `gallery-friends-error.png` and
`gallery-instance-live.png`. Review caught a misplaced friends Retry control; it was moved into the picker and both
widths were regenerated/inspected. Account, recipient and liveness states are synthetic, with existing local imagery.

## Remaining live verification

No real VRChat sign-in, personal friends request, protected image download, avatar lookup or historical-instance
liveness lookup was exercised. Routing/privacy/cache paths used mocks and local IPC fixtures. Screenshots establish
layout/cropping, not live eligibility or VRChat Join success. No live sharing, recipient delivery, Unity
installation/SDK upload, physical capture or installed-app update was performed. Malformed/missing original frame
dimensions leave older thumbnails unchanged; valid manifests are needed to locate their encoded regions.
No requested implementation remains unfinished.
