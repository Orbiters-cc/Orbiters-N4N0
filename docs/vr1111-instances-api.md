# F5/F10 instances API handoff

Branch: vr1111/instances-api. Implementation head: 48a26c3404a625a846c5fc93df43f9a17672a051 (two commits from cf908c6). A final documentation-only commit records this report in docs/vr1111-instances-api.md.

## UI interface

Use existing `controller::vrchat_data::{Scope, Request, Response}` via the existing local IPC `VrchatData` operation. Call on a worker, with `Scope::of(snapshot)` captured when opening the page. No credentials cross IPC. All four operations require `snapshot.vrchat_login.allowed && snapshot.vrchat_login.user.is_some()`. Hide Go when that is false. No UI files changed in this stream.

Requests and successful responses:

- `Request::WorldDetails { world: String }` -> `Response::WorldDetails(orbiters_core::vrchat_api::World)`. World id works independently of captures/catalog entries. Fields: id, name, author_id, author_name, description, capacity: Option<u32>, recommended_capacity: Option<u32>, tags: Vec<String>, thumbnail_image_url, image_url, visits: Option<u64>, favorites: Option<u64>. Missing numeric metadata remains None. Metadata uses the existing one-day per-session cache. The returned instances tuples are not authorization to invite; use WorldInstances.
- `Request::WorldInstances { world }` -> `Response::WorldInstances(WorldInstances { items: Vec<WorldInstance>, unavailable: usize })`. Always freshly fetches `/worlds/{world}`, then looks up each exact returned instanceId. A successful empty items list is empty, not signed out. unavailable counts hidden/missing/null lookups and MUST NOT be described as closed. Whole-operation failures propagate rather than silently appearing empty.
- `Request::CreateInstance { world, access: CreateAccess, region: InstanceRegion }` -> `Response::CreatedInstance(WorldInstance)`. Only creates, never sends an invite. `CreateAccess::{Public, FriendsPlus, Friends, InvitePlus, Invite}`; `InstanceRegion::{Us, Use, Eu, Jp}`. Creation validates returned world, access, region and personal owner (except public). Groups are deliberately absent: correct group creation needs membership/permissions/roles, outside this personal creation interface.
- `Request::InviteSelf { world, instance }` -> `Response::Invited`. Pass the EXACT id from a listed or created WorldInstance. This sends an invite, not a teleport: tell the user to accept it in VRChat. Creation and invitation are separate deliberate user operations. Do not generate a random public id or reconstruct an instance from its display number.

`WorldInstance` fields: world: String, id: String (opaque, includes access/region/nonce), access: InstanceAccess, region: String, occupants: u32, capacity: Option<u32>, friends: Option<u32>, full: bool, closed: bool, can_join: bool. `InstanceAccess::{Public, FriendsPlus, Friends, InvitePlus, Invite, GroupPublic, GroupPlus, Group, Unknown}`. Preserve an unrecognized region string for display. friends=None means unknown; Some(0) means a returned complete user list reports no friends. Friends derive only from API users[].isFriend; do not infer them from access type. Disable selection when !can_join (full, closed, unknown kind). Capacity can come from the embedded world when no instance override is supplied. A future closedAt is not already closed.

## States and errors

Pages own idle/loading/ready/empty/error/sending/sent state. Opening/loading a chooser does not create or invite. Keep world and scope with pending work, discard responses after account/environment/revision change. WorldError is a structured Response variant (the outer existing Result remains unchanged):

`WorldError::{AccountChanged, SignInRequired, InvalidWorld, NotListed, Full, Closed, NotJoinable, Api(orbiters_core::vrchat_api::Error)}`.

Api preserves Unauthorized (401/session expiry), Disabled (revoked grant), RateLimited(Duration), NotFound (hidden or absent, not closed), Unavailable, InvalidResponse and existing transport errors. Error implements Display with existing translations. For RateLimited, show retry duration; do not automatically retry mutations. No new user-facing wording is introduced here; the pages stream supplies translated labels and structured failure explanations.

On NotListed, reopen/refresh the chooser: destination permits expire after five minutes, are tied to Scope (environment, Orbiters member, VRChat user, allowed flag, revision) and exact world/id, and are retained only in the service process. Refresh clears prior permits for that world even if the refresh fails. Creating replaces that world's prior permits with the new destination. Service re-fetches selected instance before invitation and refuses full/closed/unavailable choices. VRChat may still refuse or fill the instance after that lookup; the API is final authority. Revocation checks guard actual transport and discard stale responses. A mutation already accepted remotely cannot be undone by a later scope change; do not retry it automatically.

## Implementation and evidence

Files: crates/orbiters-core/src/vrchat_api.rs (models, details, fresh enumeration, creation), vrchat_source.rs (personal identity/scoped execution without fallback), crates/orbiters-desktop/src/controller/vrchat_data.rs (IPC variants, typed errors, server permits, validation).

The bridge `create_private_instance` and private_instance_of validation are unchanged. The public_instance syntax utility remains for existing consumers but no longer authorizes IPC invites: even numeric ids must first be listed/created. The pages stream must update old random-id join calls. IPC uses the existing serialized Request/Response types, so no separate protocol file is needed.

Verified research leads: map-features F5/F10 correctly identified dropped world metadata, public-only invite guard and missing list/create requests. No contradiction found in those sections. Their tuple warning is correct: current World.instances contains [id, occupants, languageRatio], not two-element tuples. No compatibility parsing added.

Upstream specification checked read-only on 2026-11-11 (no VRChat API calls):
- https://github.com/vrchatapi/specification/blob/master/openapi/components/schemas/World.yaml
- https://github.com/vrchatapi/specification/blob/master/openapi/components/schemas/Instance.yaml
- https://github.com/vrchatapi/specification/blob/master/openapi/components/schemas/InstanceAccessType.yaml
- https://github.com/vrchatapi/specification/blob/master/openapi/components/requests/CreateInstanceRequest.yaml
- https://github.com/vrchatapi/specification/blob/master/openapi/components/schemas/CreateInstanceRequestRegion.yaml
- https://github.com/vrchatapi/specification/blob/master/openapi/components/paths/instances.yaml

Creation maps Public to public with no owner, FriendsPlus to hidden, Friends to friends, Invite/InvitePlus to private with canRequestInvite false/true. Personal owner comes from the locked signed-in client, never UI data.

## Validation

Final shared deskcargo batch: all passed. Logs: test-instances-core-final.log, test-instances-desktop-final.log, test-instances-i18n-final.log alongside this file. No new target directory was created; CARGO_INCREMENTAL=0 and the shared target were set by deskcargo.

Not verified: live API behavior, actual invite reception/acceptance, headset/page integration and macOS compilation. No real VRChat API request/invite, app launch, GPU work, push, bump, tag, release or merge performed. All six supplied screenshots were opened at original size; no visual change belongs to this stream.

Integration requirement: match Response::Invited explicitly for success. The old Worlds::join implementation (overlay/worlds/mod.rs) uses Ok(_) and would incorrectly display success for a structured WorldError. The pages stream owns replacing this call; do not release this service branch alone without that consumer update. All new callers must handle Response::WorldError as failure, in addition to the existing outer transport Result.

Final core result at implementation head 48a26c3404a625a846c5fc93df43f9a17672a051:
`deskcargo.sh <this-worktree> test -p orbiters-core vrchat_ -- --test-threads=1`: exit 0, 70 passed, 2 ignored in unit tests; live integration tests also remain ignored. The ignored tests were not enabled. New coverage includes several access kinds, fresh empty enumeration, no session, 401, 429, 500, hidden/null lookup, five creation bodies, exact opaque id in the invite URL, full/closed/unknown rows, mismatched world/id, and a Source scope change inside mocked HTTP (post-response rejection plus no HTTP for an already stale scope). Service tests cover pre/mid-work scope changes, exact world/account/revision permits, refresh invalidation and refusal of an unlisted numeric id.

Initial desktop test round: 12 passed, 2 failed because an old test expected string invite failures and the new signed-in fixture lacked the matching Orbiters identity/environment. Both assertions/fixtures were updated to the new typed contract; no production safety check was relaxed. Initial i18n round: 7 passed, exit 0. Final desktop/i18n results are below.


Final desktop result: `deskcargo.sh <this-worktree> test -p orbiters-desktop controller::vrchat_data -- --test-threads=1`: exit 0, 14 passed, 0 failed. Full desktop test binary compiled; 27 existing warnings appeared in unrelated code. No app was launched.

Final translations result: `deskcargo.sh <this-worktree> test -p orbiters-i18n`: exit 0, 7 passed, 0 failed; doc-tests 0. No new UI wording is introduced.

`git diff --check` passed. Implementation and this durable report are committed. External canonical report: C:/Users/Enzo/.orbiters-build/vr-feedback-1111/done-instances-api.md. No implementation blocker remains. Merge dependency: pages stream must consume the new typed errors and destination permits before release. No changes in other streams' UI territory.
