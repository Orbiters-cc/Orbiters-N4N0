# Optional VRChat sign-in (workstream H)

Settings has a VRChat account card only for a signed-in Orbiters member whose server grants
`n4n0-vrchat-login`. The server owns eligibility (ADMIN only during the permission-request phase);
the desktop does not infer it from a name or locally stored role.

The member can enter a VRChat username and password, complete an authenticator, email or recovery-code
challenge, see their display name and picture, and sign out. Passwords and codes are transient. Requests
run in the tray service on background threads; the window gives feedback on press and receives status
through the existing authenticated local IPC connection. The window's password is cleared on submission
or when leaving Settings. Errors stay in the card, with retry/removal actions where applicable.

## Privacy and lifecycle

- `Api::vrchat_login_feature(token)` calls `GET /features/mine?names=n4n0-vrchat-login` with only the
  Orbiters bearer credential. A grant is cached in memory for three hours. Failures deny access and
  retry after a minute; a missing, malformed or false flag never enables VRChat sign-in.
- `orbiters_core::vrchat_api::Client` owns a separate HTTP agent and private cookie state. It does not
  use the Orbiters `Api`. Its User-Agent is `Orbiters-N4N0/<version> blackorbit.eclaire@gmail.com`.
- Only `auth` and `twoFactorAuth` cookies are persisted, encrypted by current-user Windows DPAPI in
  `%LOCALAPPDATA%\Orbiters\vrchat-session.bin`. Passwords, codes and user profile data are not stored there.
  Non-Windows builds refuse persistent VRChat session storage. The existing Orbiters account store
  shares only the DPAPI utility, with a different entropy value and its own file.
- Signed-out Orbiters accounts, environment/account changes and expired feature grants revoke direct
  access. An old grant is never re-enabled. A stored VRChat session stays on disk but is not decrypted
  or used until a fresh feature check allows it. Responses from revoked requests are discarded.
- VRChat sign-out clears the local session and deletes its file before attempting `PUT /logout`.
  A network failure cannot retain the local file. A deletion failure is shown with a removal retry.
- HTTP 401 clears the session; rejected codes remain retryable. A completed verification followed by
  a failed user lookup resumes with cookies instead of consuming a recovery code again.
- The client sends at most one request per second in each endpoint family. The process-wide limiter
  survives account changes and honors both seconds and HTTP-date `Retry-After` values. HTTP 429 returns
  a retry error without automatic retries. Avatars are cached for 24 hours; friends for five minutes.
- Friends include online and offline pages of 100, deduplicated by user ID. Pagination is bounded.
- Image requests allow HTTPS on `api.vrchat.cloud`, `files.vrchat.cloud` and `assets.vrchat.com` only.
  Redirect destinations are checked before requesting them. Public `assets.vrchat.com` receives no
  session cookies and cannot set them. No VRChat credential is passed to Orbiters or arbitrary image hosts.

## Integration for the other workstreams

Obtain `Controller::vrchat_source()` in the service, then call its blocking methods on a background
worker. Do not construct a VRChat client in a page or transfer its cookies over IPC. A window using
the remote backend needs a service command/job to request these results; this workstream adds login
commands/status, not data-request IPC for the other pages.

| Consumer | Function | Result and fallback |
|---|---|---|
| Unity project avatar thumbnails | `source.avatar(avatar_id)` | `Result<Option<Avatar>>`; personal VRChat session when allowed and signed in, otherwise Orbiters `GET /vrchat/avatars/thumbnails?ids=...`. Preserve Toolkit thumbnails as the first choice. |
| Avatar/profile image bytes | `source.image(url)` | `Result<Vec<u8>>`; isolated VRChat download when signed in, otherwise the existing Orbiters `Api::image` downloader, without VRChat cookies. |
| Photos & captures friends picker | `source.friends()` | `Result<Vec<User>>`; paged, cached personal friends or the existing read-only VRCX/VRCNext data. Use `id`, `display_name` and `picture()`. |
| Instance links | `source.instance_alive(world_id, instance_id)` | `Result<Option<bool>>`; `Some(true)` for the instance VRChat answers (its `active` when given), `Some(false)` when it reports it closed (`closedAt`, `hardClose`, `active: false`) or no longer knows it (404 "Instance not found", or `null`). `None` means inaccessible (403), not closed. |
| Profile lookup | `source.user(user_id)` | `Result<Option<User>>`; direct lookup when available; no substitute profile otherwise. |
| Current VRChat user | `source.current_user()` | `Result<Option<User>>`; refreshes the direct session. |
| Overlay world search | `source.worlds(list, page)` | `Result<FoundWorlds>`; a page of a search, popular, active or favorite worlds through the personal session (kept five minutes), else Orbiters' public `GET /maps/search` / `/maps/featured` (Active and Favorites need the session). `Request::WorldList` (docs/world-view-overlay.md). |
| Join a world from the overlay | `source.invite_self(world, instance)` | `Result<()>`; an invite to oneself to a public instance, personal session only (`Request::InviteSelf`, checked to be a public instance first). |

Callers should use a fresh source for each operation. Direct 401 or feature revocation falls back;
rate limits and service errors are surfaced rather than adding load to the shared account. Signing
out or losing eligibility also invalidates cached direct data. The UI's signed-in display is in
`Snapshot::vrchat_login`; it never contains cookies or a password.

The Unity project thumbnails, sharing friends picker and instance-info dialog now use this boundary through
read-only `VrchatData` IPC requests. Each request runs on a service worker and is answered only to its requesting
window, with a correlation ID. Account, environment, eligibility or session changes discard pending results and
clear the window's direct-data caches. Image loaders also call the service; cookies never leave it.

Toolkit avatar files remain first. A signed-in personal session loads the remaining avatars and image bytes;
without one, the existing batched Orbiters thumbnail cache/request path is retained. Personal friends are merged
with the local VRCX/VRCNext list by `usr_` ID, with personal names and pictures preferred. Public local-cache pictures
can still load without signing in to Orbiters. Direct service/rate-limit errors are shown with retry actions.

Opening Instance info lazily checks that world/instance. The location goes in the path as VRChat's log writes it
(`/instances/wrld_…:25332~hidden(usr_…)~region(eu)~nonce(…)`, `vrchat_api::path_segment`): VRChat's firewall refuses
its `( ) , :` percent-encoded with 400 `{"error":{"message":"\"malformed url\"","waf_code":26497}}`, which was shown as
"VRChat returned an unexpected answer" (0.3.4-dev.3). An error status a request does not expect is now
`Error::Refused(status, VRChat's message)` ("VRChat refused the request (HTTP 400: malformed url)."), the whole of it on
hover in the popover; a closed or unknown instance shows "Not open any more". The client caches explicit true/false and unknown results
for 60 seconds (at most 256 entries); revocation prevents even cached reads. Explicitly active history instances
offer Open in VRChat; explicitly inactive instances do not. Unknown or inaccessible instances retain the existing
current-instance behavior. No server feature row, version, release, push or live VRChat sign-in is included here.

## Changing an avatar's or world's picture

The one write. In the Unity projects panel (Avatars and Worlds cards), a thumbnail of an item the signed-in VRChat
account uploaded dims under an upload mark on hover ("Change the picture on VRChat"); a press opens Windows' file dialog
on a worker and the window sends `ToService::VrchatPicture { scope, id, path }` (windows only, not the overlay).
`Controller::vrchat_picture` (controller/vrchat_data.rs) refuses without a signed-in session or while another picture
uploads, then on its own thread checks the file (a file, 1 byte to 50 MB, PNG/JPEG/WebP/GIF, at least 128 px a side),
turns it by its EXIF orientation, crops it about its centre to 4:3 (VRChat shows these pictures edge to edge, so bars
would become part of the picture), makes it 1200 × 900, flattens transparency over black and encodes a PNG. Progress
and outcome are in `Snapshot::vrchat_login.upload` (a serial per upload), the outcome a toast too. On success the
copies kept on this computer are replaced where they exist (the Toolkit's `Avatars/<id>.png`, Orbiters' cached
`avatar-thumbnails`, the world's `world-thumbnails` entry) and the page shows the new picture at once.

`vrchat_api::replace_image` does what VRChat's SDK (librsync.net) and VRCX do, locking the client per request only:
a fresh `GET /avatars/{id}` or `/worlds/{id}` whose `authorId` must be the session's user (else nothing is sent);
`GET /file/{fileId}` of the current `imageUrl`, reused for a new version when it is the user's `image/png` file with a
complete last version, else `POST /file {name, mimeType: image/png, extension: .png}` (nothing is ever deleted);
`POST /file/{fileId}` with base64 `fileMd5`/`signatureMd5` and sizes; for `file` then `signature`:
`PUT …/{version}/{part}/start` → a presigned URL, a cookie-less `PUT` of the bytes with `Content-Type` and base64
`Content-MD5` (HTTPS `*.amazonaws.com` or `files.vrchat.cloud` only, default port, no userinfo; its cookies are never
read), `PUT …/finish {"maxParts":"0","nextPartNumber":"0"}`; `GET /file/{fileId}` until the version is `complete`
(about a minute at most); `PUT /avatars/{id}` or `/worlds/{id}` `{id, imageUrl}` with
`https://api.vrchat.cloud/api/1/file/<file>/<version>/file`, which the answer must echo. The signature is librsync's
BLAKE2 format: magic `0x72730137`, block length 2048, strong-sum length 32, then per block the rollsum and
BLAKE2b-256, big-endian. MD5 and BLAKE2b are implemented in `vrchat_api` with reference vectors; the expected
signatures come from a Python transcription of librsync.net. `Request::Owned { ids }` (16 at a time) tells the page
which thumbnails are editable, from the client's day-long avatar/world caches (`authorId`).

## Validation

All Cargo commands use `CARGO_TARGET_DIR=C:/Users/Enzo/.orbiters-target` and `CARGO_INCREMENTAL=0`.

- `cargo test -p orbiters-core --no-run`: compiled the crate's test targets.
- `cargo test -p orbiters-core --lib`: 125 passed, six existing live/machine-specific tests ignored.
- `cargo test -p orbiters-core --lib vrchat_source::tests`: the authenticated mock session expires;
  the fallback request contains the Orbiters bearer credential and no VRChat cookie.
- `cargo test -p orbiters-desktop controller::vrchat_login::tests`: four tests for redaction,
  signed-out denial, immediate revocation and isolation of previous grants.
- `cargo check -p orbiters-desktop` and the screenshot build compile successfully. Existing unused
  upload-method warnings remain; `vrchat_source` remains unused until the coordinator wires consumers.

Unit tests use a mocked VRChat transport, loopback Orbiters responses and disposable encrypted test
files. They cover Basic-auth encoding, all three second-factor methods, cookies, DPAPI, rotation,
restore/logout failure, feature checks/expiry, rate limiting, pagination, cache expiry and redirects.
No real request to VRChat and no real credential were used.

Offscreen visual review:

```powershell
cargo run -p orbiters-desktop -- --screenshot C:/Users/Enzo/.orbiters-vrchat-login-qa/final --sessions C:/Users/Enzo/.orbiters-vrchat-login-qa/sessions --only vrchat-login
```

The `final` directory contains 14 inspected `vrchat-login-*.png` images: signed-out, entering-password,
totp, email, recovery, signed-in, error, busy, disabled, orbiters-signed-out, code-error, rate-limited,
unavailable and remove-error. `compact/vrchat-login-entering-password.png` checks the 900-by-760 layout.
These are synthetic account states, not evidence of a real authentication or remote logout.

Authentication and endpoint shapes were checked against the community specification on 2026-10-29:
[authentication](https://github.com/vrchatapi/specification/blob/main/openapi/components/paths/authentication.yaml),
[friends](https://github.com/vrchatapi/specification/blob/main/openapi/components/paths/friends.yaml),
[instances](https://github.com/vrchatapi/specification/blob/main/openapi/components/paths/instances.yaml).
