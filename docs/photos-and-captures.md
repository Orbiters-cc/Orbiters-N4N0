# Photos & captures

Photos & captures combines this computer's VRChat pictures and N4N0 recordings/3D captures with the signed-in
member's `/captures` feed. Mine, All and Shared with me select ownership; Search worlds filters local names and
sends `q` to Orbiters. More items use the server's opaque cursor. Overlapping pages are deduplicated; a repeated
cursor stops pagination. Results and filtered indices are retained between frames.

Items are ordered newest first and grouped by world and recorded instance. A session's manifest supplies its
instance; pictures use the most recent retained VRChat join before their timestamp, only when it names the same
world. Items without an instance are grouped by world/day and say that the instance was not recorded. Shared items
currently have no instance field in the contract. Instance info reads VRCX's recorded joins/leaves on a background
thread for a historical roster when available. Open in VRChat appears only for the currently running VRChat
instance: historical joins and the Previous instances list do not establish that an instance is still alive.

Manage captures retains reconstruction, import, storage cleanup and the original detailed capture inspector.
The main gallery opens an item dialog. Local reconstructed captures use the existing 3D viewer; downloaded `.splat`
captures use the same renderer, with the contract's starting view and a validated `.splat` reader. Pictures open a
larger view. Downloaded files are saved under `Documents/Orbiters/Shared captures` with unique names. Protected
picture/file requests carry the Orbiters credential to the capture file endpoint; thumbnails are decoded off the UI
thread. Existing picture loaders also accept PNG, JPEG and WebP remote profile pictures.

## Sharing and revoking

- Share publicly uploads an unuploaded item publicly, or shares an uploaded item with `public: true`.
- Share to … opens a searchable VRChat friends picker, with cached names and pictures from VRCX/VRCNext, up to
  50 recipients and a 280-character optional message. New uploads are private before their grants are made.
- Recipients see who shared the item, Download / open and Request share. A request can name recipients and/or ask
  for public sharing. A 202 response is shown as a pending request, never as a successful publication.
- Owners can unshare publicly, revoke a person or unshare everything. Re-sharers see and can revoke the grants the
  server returns for them. Revocation is shown immediately; a failure restores the saved item and offers Retry.
- Failures stay in the dialog, including in the picker. Busy actions disable immediately. A 409 with code
  `explicit-acknowledgement` displays the server message and Share anyway / Cancel; the retry sets
  `acknowledgeExplicit: true`. A locally flagged item shows this warning before its public request.

Visited worlds cards and capture rows share the same dialog. Share expands into Share publicly and Share to …
with animated width, label fades and separating halves. Its spring matches the website sign-in button's
340 stiffness / 25 damping / 0.8 mass. Pointer down acts immediately; outside press and Escape collapse it. A
world's card chooses its newest reconstructed capture, or its pictures when it has no reconstructed capture;
individual captures and selected pictures are also available from the world's detail.

Share, revoke and download mutations are submitted over IPC to the tray controller. Its bounded job history is
part of the published snapshot; the dialog shows progress immediately and reconciles the controller's result.
Closing a window leaves the job running in the tray, with a completion notice waiting for the next window.
Automatic updates regard an unfinished sharing job as an upload in progress.

Uploads are remembered in `%APPDATA%/Orbiters/Desktop/capture-uploads-<dev|prod>-<member id>.json`. Each record binds
a local path to the returned `type`, `id`, `visibility` and latest Item. The upload identity is written before
fetching its Item or making grants: an interrupted grant request can retry without uploading the file again.
The account/environment scope prevents one member's upload identities being used with another account.
The controller reports the current VRChat world/instance/display name on joins and every five minutes, with a
60-second retry delay after failures, for the contract's shared-capture world listing.

## Capture-time content evidence

The desktop writes `capture-content.json` next to `manifest.json` when the capture worker starts a 3D picture or
recording. It is an atomic JSON sidecar:

```json
{
  "explicit": true,
  "explicitReasons": ["osc-parameter:NSFW/Toggle", "avatar-label:content_sex"]
}
```

The current worn avatar's known OSC parameter **names**, including parameters whose value is false, are matched
case-insensitively for `nsfw`, `nude`, `naked`, `sps`, `dps`, `tps`, `penetrat`, `orifice`, `genital` and `lewd`.
Reasons retain the original name without `/avatar/parameters/`. The app's existing OSC/OSCQuery path supplies these
names and forgets the previous avatar's parameters on avatar change.

VRCNext's database is opened read-only. `user_tracking.profile_current_avatar_id` joins `avatar_tracking.tags` only
for users actually in the current VRChat log roster and whose cached `profile_location` exactly matches the
current world/instance. Real `content_sex` tags and the contract's `sexually_explicit` label are recognized, and the
raw tag is retained in the reason. [VRChat describes the sexual-content warning](https://hello.vrchat.com/blog/content-gating).
The inspected VRCX `cache_avatar` has no content-label field; its user-authored `avatar_tags` are not treated as
VRChat content warnings. Avatar names/descriptions are never used to guess explicit content.

Pictures whose timestamps fall inside a flagged session inherit its evidence when the gallery is indexed.
Both public and private uploads carry `explicit` and JSON `explicitReasons` multipart fields, alongside visibility
and the existing metadata. An absent cache/OSC signal is unavailable evidence; it cannot prove that an item is safe.
Existing recordings are not retroactively classified.

## Contract assumptions and verification limits

The binding contract remains `contracts/Docs/capture-sharing.md`, unchanged by this workstream. It does not specify
ShareRequest fields or the upload response envelope. The client preserves ShareRequest JSON as an opaque value,
expects preview upload identity at the response root, and expects picture identities in `added: [{type,id,visibility}]`
with a numeric `skipped`. The single-item route is read as a bare Item. These envelope assumptions must be checked
against the development server before release. An already-skipped picture without a returned id raises an inline
error asking for a gallery refresh; the client does not guess its identity. The contract also has no local source
identity or instance on Item, so previously uploaded local files without an upload record may appear separately
from their server copies, and shared-instance details cannot be reconstructed from that Item alone.

Unit fixtures cover contract JSON, requests, 409 warnings, multipart fields, cursor encoding/merging, revocation
and instance association; the downloaded-splat test covers coordinates and malformed input. Offscreen screenshots
use the checked-in JSON fixture and local thumbnail imagery. They do not establish real server permissions,
upload/public replacement behavior, request approval, grant cascades, VRChat slot allocation, running capture
evidence, live download/rendering or in-client behavior. No production uploads, version bumps, tags, pushes or
releases are part of this implementation.
