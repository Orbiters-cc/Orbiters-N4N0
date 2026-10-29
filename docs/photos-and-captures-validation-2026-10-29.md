# Photos & captures implementation validation — 2026-10-29

This is local implementation and fixture/offscreen validation. The coordinator owns the release and contracts
submodule pointer. No version bump, tag, push, release or production upload was performed.

## Implementation

- `767131f` — Add capture-sharing transport, persistence, and capture-time content flags.
- `8148830` — Build Photos & captures with durable sharing and revocation.
- The follow-up incorporates contracts `a2d12ca` / `c9025c2`: wrapped single Item responses, typed share requests,
  incoming/outgoing approval UI, explicit public-approval acknowledgement, source keys, instance attribution,
  owner/local deduplication, and manifest `nsfw` tags.

The implementation guide is [photos-and-captures.md](photos-and-captures.md). Main files:

| Area | Files |
| --- | --- |
| Contract client and evidence | `crates/orbiters-core/src/shares.rs`, `explicit.rs`, `maps.rs`, `vrchat_accounts.rs`, `lib.rs` |
| JSON fixtures | `crates/orbiters-core/tests/fixtures/captures.json`, `capture-requests.json` |
| Background/tray operations | `crates/orbiters-desktop/src/controller/shares.rs`, `controller/maps.rs`, `controller.rs`, `backend.rs`, `ipc.rs`, `service.rs`, `state.rs` |
| Gallery and dialogs | `crates/orbiters-desktop/src/pages/gallery.rs`, `sharing.rs`, `requests.rs`, `sessions.rs`, `worlds.rs`, `splats.rs`, `settings.rs`, `mod.rs` |
| Navigation, pictures, screenshots | `crates/orbiters-desktop/src/app.rs`, `worlds.rs`, `screenshot.rs` |
| Downloaded splat decoding | `crates/orbiters-reconstruct/src/web.rs` |
| Dependency metadata | `Cargo.lock`, `crates/orbiters-desktop/Cargo.toml` |

Capture and Unity projects pages were left to their workstream. Shared controller and screenshot edits are staged
as individual hunks, preserving the concurrent VRChat sign-in work.

## Checks

All Cargo commands used:

```powershell
$env:CARGO_TARGET_DIR = 'C:/Users/Enzo/.orbiters-target'
$env:CARGO_INCREMENTAL = '0'
```

| Command | Result |
| --- | --- |
| `cargo check -p orbiters-desktop` | Passed after sharing/request integration; existing unused-backend warnings remain. |
| `cargo test -p orbiters-core shares::tests --lib` | 10 passed: JSON, paging, ownership/revocation, request 202, 409 warning, multipart fields, wrapped Item/nullable attribution, typed requests and explicit approval/denial, plus exact source-key lookup. |
| `cargo test -p orbiters-core explicit::tests --lib` | 3 passed: parameter matching, real cached content tags/current roster, manifest tags. |
| `cargo test -p orbiters-desktop controller::shares::tests -- --nocapture` | 4 passed: instance attribution, source-key ownership/kind/world matching, tray-job lifetime/IPC, content sidecar at a synthetic worker start. |
| `cargo test -p orbiters-desktop pages::gallery::tests -- --nocapture` | 1 passed: known instances cross midnight; world ids and unknown days remain separate. |
| `cargo test -p orbiters-desktop ipc::tests -- --nocapture` | 3 passed. |
| `cargo test -p orbiters-reconstruct downloaded_splats --lib` | 1 passed: coordinates, rotation and malformed input. |
| `cargo test -p orbiters-core --lib` (earlier checkpoint) | 97 passed, 6 ignored, before the final contract additions. |
| `cargo test -p orbiters-desktop state::tests -- --nocapture` | No tests matched; updater-busy coverage is in the tray-job test. |

Formatting was limited to named crates/files, with `skip_children=true` on shared module roots so Capture/Unity
projects files were not formatted by this workstream. No H: target folder was built into or deleted.

## Screenshots

```powershell
cargo run -p orbiters-desktop -- --screenshot C:/Users/Enzo/.orbiters-sharing-qa `
  --sessions C:/Users/Enzo/Documents/Orbiters/Sessions --only sharing
```

The PNGs are under `C:/Users/Enzo/.orbiters-sharing-qa/`:

```text
gallery-mixed.png
gallery-mine.png
gallery-shared.png
gallery-search.png
gallery-owner.png
gallery-public.png
gallery-grantee.png
gallery-friends.png
gallery-explicit.png
gallery-error.png
gallery-picture.png
gallery-progress.png
gallery-request.png
gallery-instance.png
gallery-empty.png
gallery-requests.png
gallery-requests-explicit.png
gallery-requests-approved.png
gallery-requests-error.png
gallery-requests-outgoing.png
gallery-requests-denied.png
world-share-mid.png
world-share-open.png
world-share-detail.png
```

The images use contract JSON fixtures with local thumbnail imagery. They were opened and compared with Visited
worlds' existing surfaces and controls. Review led to moving the friends picker into the right column, keeping
surfaces visible through the Share morph, giving request cards the full panel width, and sizing the request list
so its decisions and inline retry errors remain accessible.

Two additional 920 × 640 renders were opened and reviewed under
`C:/Users/Enzo/.orbiters-sharing-qa-narrow/`: `gallery-mixed.png` and `gallery-requests.png`.
At this width the search moves to its own row; the Requests list scrolls within the dialog.
They were rendered with the same command, adding `--width 920 --height 640` and selecting each state with `--only`.

## Evidence and remaining verification

`capture-content.json` is written atomically beside `manifest.json` at worker start. Its format is:

```json
{"explicit":true,"explicitReasons":["osc-parameter:NSFW/Toggle","avatar-label:content_sex","manifest-tag:nsfw:Body/Hidden toggle"]}
```

VRCX/VRCNext schemas were inspected read-only. VRCX's inspected avatar cache has no VRChat content labels; VRCNext's
real cached `content_sex` tag is joined through current user avatar ids, exact location and the live log roster.
Manifest controls tagged `nsfw` and matching final parameter names are also read for the worn avatar. Missing
evidence does not establish non-explicit content, and existing sessions are not retroactively classified.

No known deviation remains from the updated sharing contract. Nullable instance/source keys are accepted while
the server work lands. Retained joins do not establish historical liveness; the integration follow-up now checks
opened instance groups through the member's eligible VRChat session and offers Open in VRChat for explicit active
results. Unknown results preserve the current-instance restriction. A skipped upload without an identifiable returned Item
can still require a gallery refresh.

Still unverified against a live development server: real upload and public replacement, private-file access and
downloads, incoming/outgoing requests and approval/denial, grant cascades, moderation, presence/slot allocation,
and rendering a server-downloaded capture. Also unverified: physical VRChat capture-time evidence, live avatar
switches, friend-recipient delivery and VRChat client behavior. Synthetic events, read-only databases, HTTP
fixtures and offscreen frames do not replace these checks.
