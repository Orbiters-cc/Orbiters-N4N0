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
| Instance links | `source.instance_alive(world_id, instance_id)` | `Result<Option<bool>>`; `Some(true/false)` only when VRChat explicitly reports `active`. `None` means unverified or inaccessible, not closed. Preserve the existing link behavior. |
| Profile lookup | `source.user(user_id)` | `Result<Option<User>>`; direct lookup when available; no substitute profile otherwise. |
| Current VRChat user | `source.current_user()` | `Result<Option<User>>`; refreshes the direct session. |

Callers should use a fresh source for each operation. Direct 401 or feature revocation falls back;
rate limits and service errors are surfaced rather than adding load to the shared account. Signing
out or losing eligibility also invalidates cached direct data. The UI's signed-in display is in
`Snapshot::vrchat_login`; it never contains cookies or a password.

The existing project pages, sharing pages, friend picker and instance links are intentionally left
for their workstream owners to connect to this boundary. No server feature row, version, release,
push or live VRChat sign-in is included in this local change.

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
