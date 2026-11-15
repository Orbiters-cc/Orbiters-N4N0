# F5/F10 instances API handoff

Branch: vr1111/instances-api. Implementation head: 48a26c3404a625a846c5fc93df43f9a17672a051 (two commits from cf908c6). A final documentation-only commit records this report in docs/vr1111-instances-api.md.

## UI interface

Use existing `controller::vrchat_data::{Scope, Request, Response}` via the existing local IPC `VrchatData` operation. Call on a worker, with `Scope::of(snapshot)` captured when opening the page. No credentials cross IPC. All four operations require `snapshot.vrchat_login.allowed && snapshot.vrchat_login.user.is_some()`. Hide Go when that is false. No UI files changed in this stream.

Requests and successful responses:

- `Request::WorldDetails { world: String }` -> `Response::WorldDetails(orbiters_core::vrchat_api::World)`. World id works independently of captures/catalog entries. Fields: id, name, author_id, author_name, description, capacity: Option<u32>, recommended_capacity: Option<u32>, tags: Vec<String>, thumbnail_image_url, image_url, visits: Option<u64>, favorites: Option<u64>. Missing numeric metadata remains None. Metadata uses the existing one-day per-session cache. The returned instances tuples are not authorization to invite; use WorldInstances.
- `Request::WorldInstances { world }` -> `Response::WorldInstances(WorldInstances { items: Vec<WorldInstance>, unavailable: usize })`. Always freshly fetches `/worlds/{world}`, then looks up each exact returned instanceId. A successful empty items list is empty, not signed out. unavailable counts hidden/missing/null lookups and MUST NOT be described as closed. Whole-operation failures propagate rather than silently appearing empty.
- `Request::CreateInstance { world, access: CreateAccess, region: InstanceRegion }` -> `Response::CreatedInstance(WorldInstance)`. Only creates, never sends an invite. `CreateAccess::{Public, FriendsPlus, Friends, InvitePlus, Invite}`; `InstanceRegion::{Us, Use, Eu, Jp}`. Creation validates returned world, access, region and personal owner (except public). Groups are deliberately absent: correct group creation needs membership/permissions/roles, outside this personal creation interface.
- `Request::InviteSelf { world, instance }` -> `Response::Invited`. Pass the EXACT id from a listed or created WorldInstance. This sends an invite, not a teleport: tell the user to accept it in VRChat. Creation and invitation are separate deliberate user operations. Do not generate a random public id or reconstruct an instance from its display number.

`WorldInstance` fields: world: String, id: String (opaque, includes access/region/nonce), access: InstanceAccess, region: String, occupants: u32, capacity: Option<u32>, friends: Option<u32>, full: bool, closed: bool, can_join: bool. `InstanceAccess::{Public, FriendsPlus, Friends, InvitePlus, Invite, GroupPublic, GroupPlus, Group, Unknown}`. Preserve an unrecognized region string for display. friends=None means unknown; Some(0) means a returned complete user list reports no friends. Friends derive only from API users[].isFriend; do not infer them from access type. Disable selection when !can_join (full, closed, unknown kind). Capacity can come from the embedded world when no instance override is supplied. A future closedAt is not already closed. Closed means `closedAt` past or `hardClose: true` only: `active: false` is what VRChat answers for an instance nobody has entered yet, every new one included (measured 2026-11-15, L-invite-error), and is joinable.

## States and errors

Pages own idle/loading/ready/empty/error/sending/sent state. Opening/loading a chooser does not create or invite. Keep world and scope with pending work, discard responses after account/environment/revision change. WorldError is a structured Response variant (the outer existing Result remains unchanged):

`WorldError::{AccountChanged, SignInRequired, InvalidWorld, NotListed, Full, Closed, NotJoinable, BridgeBusy, NotStarted(String), Api(orbiters_core::vrchat_api::Error)}`.

Api preserves Unauthorized (401/session expiry), Disabled (revoked grant), RateLimited(Duration), NotFound (hidden or absent, not closed), Unavailable, InvalidResponse and existing transport errors. Error implements Display with existing translations. For RateLimited, show retry duration; do not automatically retry mutations. No new user-facing wording is introduced here; the pages stream supplies translated labels and structured failure explanations.

On NotListed, reopen/refresh the chooser: destination permits expire after five minutes, are tied to Scope (environment, Orbiters member, VRChat user, allowed flag, revision) and exact world/id, and are retained only in the service process. Refresh clears prior permits for that world even if the refresh fails. Creating adds the new destination beside that world's listed permits (it no longer replaces them). Service re-fetches selected instance before invitation and refuses full/closed/unavailable choices. VRChat may still refuse or fill the instance after that lookup; the API is final authority. Revocation checks guard actual transport and discard stale responses. A mutation already accepted remotely cannot be undone by a later scope change; do not retry it automatically.

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


## Review fixes

All four findings in review-instances-api.md were checked against dfdde538f66a80ef9460eda676f4e82f960ce3ef and confirmed. Work stayed in the vr1111/instances-api worktree. No push, release, version bump, tag, installed-app launch, headset control, settings change or GPU workload.

1. **Must fix: sign-out acceptance and queued mutations.** Gate now has a separate operation generation. Accepting a login command invalidates old operation tokens synchronously; replacing/signing out of the session also clears the snapshot user immediately and advances its revision. Personal operations capture authorization before waiting for the client lock and check it after locking, throughout pacing, and immediately before HTTP dispatch. Logout retains its separate feature authorization. The pacing test pauses deterministically at a barrier, revokes authorization, then releases it and asserts zero creation/invite POSTs. The Controller test accepts the actual SignOut command while holding the client mutex, checks immediate scope invalidation/user removal, then releases the lock and proves the previously captured operation cannot POST. Already dispatched remote work cannot be undone. Tests: review_revocation_during_pacing_prevents_create_and_invite_dispatch and review_signout_acceptance_invalidates_queued_create_before_unlock.

2. **Must fix: Go false success.** The existing consumer now reports success only for Response::Invited; WorldError(NotListed), an unrelated Ok response and transport errors are failures. The signed-out hit target and launcher fallback are removed, with a stale-action guard as well. The existing panel interaction test covers both signed-in and signed-out states. Tests: typed_refusal_is_never_invite_success and a_world_without_a_capture_opens_its_page_to_join_it. This is a safety fix to the legacy consumer, not a duplicate chooser implementation. The legacy numeric destination is still refused by permits until the pages chooser replaces that entry point. Source inspection of vr1111/pages at 2d1b9dbe confirms its chooser already uses selected/created exact IDs, handles WorldError and matches Invited explicitly. Combined UI compilation and headset behavior are not established by that inspection.

3. **Should fix: unbounded enumeration / work after IPC timeout.** Listings return at most 16 lookups per request and release the client lock between detail lookups. Continuation IDs must match the server-provided pending list for the exact scope/world, expire after five minutes, and are invalidated on refresh. Requests get a 60-second dispatch deadline at service acceptance, before worker scheduling and lock contention. It is checked after locking and pacing; the IPC wait remains 120 seconds, the existing HTTP timeout remains 30 seconds, and API pacing remains one second. DeadlineExceeded identifies an expired operation that cannot dispatch a later mutation. OutcomeUnknown identifies transport/server/invalid-success-response failures after a mutation was dispatched: check VRChat before retrying. Both messages have all five languages. Mock tests cover a 122-instance list (16 results plus 106 remaining IDs), continuation validation/advancement and refusal of an expired creation without HTTP. Tests: review_large_listing_is_bounded_and_preserves_remaining_ids, review_continuation_uses_server_ids_and_releases_bounded_batches, and review_expired_queued_create_never_posts_and_permits_expire. These are mock/state bounds, not live latency measurements.

4. **Should fix: complete service-path coverage and coherent fixtures.** A dev-only core test-support feature supplies an in-memory HTTP transport and a client with no session store. Desktop tests now exercise Controller list/create -> permit -> fresh lookup -> exact self-invite POST, fresh full/closed/null results, 401/429 during creation and invitation, 500 unknown invitation outcomes, scope/account/user/environment/revision rejection, permit expiry, and sign-out cancellation. Tests include review_service_list_create_fresh_lookup_and_invite, review_service_fresh_refusals_and_transport_errors, review_invite_post_errors_are_not_success_and_scope_changes_do_no_http, review_changed_account_environment_and_user_refuse_before_http, and review_all_access_kinds_preserve_coherent_opaque_ids. Core fixtures now distinguish public, hidden, friends, private/requestable and private IDs, with nonces on non-public IDs; all three group types have their exact groupAccessType tags. Future closedAt, embedded-world capacity and unknown region were already implemented correctly; explicit tests now protect those behaviors. Fixture grammar was checked against https://github.com/vrchatapi/vrchat.community/blob/main/content/docs/%28guides%29/instances.mdx .

**Exact public interface additions required by finding 3:**
- Request::WorldInstancesNext { world: String, instances: Vec<String> }.
- WorldInstances.remaining: Vec<String>. WorldInstances now returns the first bounded batch; existing request variants and response variants retain their names/shapes otherwise. Pass the previous response's remaining vector unchanged to WorldInstancesNext. Append returned items and accumulate unavailable; an empty items vector with nonempty remaining is not an empty world. Continue through a separate UI request/worker turn, preserving scroll and selection. Refresh starts with WorldInstances and invalidates earlier permits/continuations. Any WorldInstances struct literals need remaining (or ..Default::default()).
- vrchat_api::Error::{DeadlineExceeded, OutcomeUnknown}; pages go_words already delegates WorldError::Api(e) to its translated Display.
- Additive core dispatch helpers Operation (including with_deadline), Gate::{operation,cancel_operations}, Client::{operation,with_operation,world_instance_ids,world_instance_batch}, Source::{scoped,personal_until}. Existing Source::personal and Controller::vrchat_data signatures are retained. Controller::vrchat_data_until is crate-internal.

**Merge requirement:** pages at the inspected commit does not consume remaining/WorldInstancesNext. Its chooser must expose progressive batches and update any WorldInstances literals. Preserve its chooser when resolving worlds/mod.rs and panel.rs conflicts; retain explicit Invited matching, typed error handling, signed-out hiding and acceptance-time scope invalidation. Do not release this service branch alone as the completed F5 chooser. Other-stream territory touched minimally: controller/account.rs and vrchat_login.rs (session acceptance plus fake account setup), service.rs (deadline at acceptance), worlds/mod.rs and panel.rs (safe consumer), panel/shots.rs (signed-in/out scenarios), i18n catalogs, and dev-dependency declarations.

**Regression procedure:** review-red-core.log and review-red-desktop.log run the new assertions with the reviewed broken dispatch, enumeration, sign-out and success-match behavior reinstated, retaining the new test seams. Core failed the unbounded-list and revoked-mutation tests; desktop failed acceptance-time sign-out, expired dispatch and typed-refusal success. Initial test harness issues were corrected: the fake Orbiters runtime needed its server marker, and the URL assertion must compare the actual path_segment contract (parentheses are permitted), not assume they are percent-encoded. The corrected fixed-source runs below are authoritative. The red core run exited 101 (2 passed, 2 failed). The corrected-fixture red desktop run exited 101 (13 passed, 4 failed): the three intended regressions plus the URL-encoding assertion described above. No test was weakened to accept an unsafe operation. Translation coverage first reported 6 passed / 1 failed because a Windows encoding conversion changed the existing invite ellipsis into a question mark; the existing translated key was restored and the final coverage run passed.

**Verification results:**
- Shared queue: deskcargo.sh <this-worktree> test -p orbiters-core -p orbiters-desktop -- vrchat_ overlay::worlds --test-threads=1: exit 0; core 74 passed / 2 ignored, desktop 106 passed / 2 ignored. Live core integration tests: 2 remain ignored; other integration/doc tests were filtered or had no matching tests. Log: review-green-modules.log. Both must-fix regressions and the continuation/deadline/service-workflow tests pass.
- deskcargo.sh <this-worktree> test -p orbiters-i18n: exit 0; all 7 coverage tests passed, doc-tests 0. Final log: review-final-i18n.log.
- deskcargo.sh <this-worktree> build -p orbiters-desktop: exit 0, 55.28 seconds after obtaining the lock. Final log: review-final-build.log. The build has 33 warnings in existing unrelated code; no warning cleanup was included.
- Software-only --world-view <out> --panel-only --language <tag>, en/fr/ja/es/pt-BR: all five exit 0. Files: shots-instances-api-review/<language>/panel-review-signed-in.png and panel-review-signed-out.png. All ten opened at original size: invite labels fit and the button disappears signed out. The final regenerated ten files match the inspected SHA-256 hashes (zero mismatches). Original six REQUEST screenshots were also opened at original size. These renders establish static button visibility/text, not VR composition or interaction.
- git diff --check: exit 0. No test was enabled that needs a GPU, a live session or a headset. No new cargo target directory was created. Module tests preceded only restoration of the existing translated ellipsis key and comment corrections; final translation coverage and the final production-mode desktop build include that correction.

**Not verified:** live authenticated VRChat creation/invite reception, remote mutation cancellation after dispatch, headset interaction/comfort, macOS compilation, or the combined pages branch with the new continuation interface.


Final review head is recorded in the canonical external done-instances-api.md after this commit.

## Going there directly (L-go-direct, 2026-11-15)

`Request::GoThere { world, instance }` (same permit and fresh lookup as InviteSelf) answers `Response::Went(travel::Went)`:
`Direct` (the instance's `vrchat://launch` link written to VRChat's link pipe `\.\pipe\VRChatURLLaunchPipe`, served by the
person's own VRChat), `Started` (no VRChat ran: VRChat's launcher with the link and `--profile=0`), or `Invited(Detour)` where a
direct trip cannot be: `OtherVrchat` (the pipe's server, asked with `GetNamedPipeServerProcessId`, is a VRChat of Orbiters' bridge:
its main session, a side session, or any VRChat whose `--osc` port is not 9000; nothing is written to it), `LinkRefused` (the pipe
could not be opened or written), `Remote` (VRChat runs on another device). Only the bridge's main VRChat running: `BridgeBusy`,
nothing sent. `InviteSelf` stays for "Send me an invite instead" (`Detour::Asked`). Code: `controller/travel.rs`. The page's card
says how and, from VRChat's log, when the person is in the instance.
