# The VRChat bridge (experimental)

An MCP service in the tray process that lets an agent such as Claude Code start VRChat in desktop mode, go from world to world in
private instances, look at the screen, walk around, take 3D pictures, see them finish, and close the game. The code is in
`crates/orbiters-desktop/src/bridge/`, with its real game in `controller/bridge_game.rs` and Settings' section in
`pages/experimental.rs`.

## What VRChat's rules say about this

VRChat's [Terms of Service](https://hello.vrchat.com/legal) (Effective Date: October 23, 2026), section 13.2, say that you agree not to:

> i. bypass any security or other features of the Platform designed to control how the Platform is used, harvest, or mine User
> Content from the Platform, or otherwise access or use the Platform in a manner inconsistent with individual human usage;
>
> j. use any engine, software tool, agent, device, or mechanism (including any robot, spambot, spider, crawler, scraper, or other
> automated means or interface) not provided by us to access, search, or otherwise use any portion of the Platform or to extract data;

An agent driving the game is "an agent" and "automated means", it uses the game "in a manner inconsistent with individual human
usage", and capturing other creators' worlds in bulk can be read as harvesting User Content. **This feature is against those clauses as
written, and running it puts the VRChat account at risk**: "If you violate any provision of these Terms, your permission from us to use the
Platform will terminate automatically", VRChat "may in its sole discretion terminate your User Account [...] at any time for any reason or no
reason, with or without notice", and section 13.4 "reserves the right to monitor your use of the Platform (whether or not you are in a
private world, location or setting within the Platform)". Private instances reduce the harm to other people to about nothing; they do not
make it allowed. OSC and launch options being documented features does not change what automating the client with them is.

The bridge does nothing to hide from VRChat: no evasion, no humanised timing, nothing around Easy Anti-Cheat, no touching of the game's
memory, no injection. Settings says the same where it is turned on.

## Who has it

The server decides: `GET /features/mine?names=n4n0-vrchat-bridge`, on the server the build talks to (`Server::current()`), with the signed-in
Orbiters account. The backend's `checkFeatureAccessBatch` gives owners every feature and everybody else only what a `Feature` row
grants; no row names this one, so it is owners only, on production and on development, with no server change. A row would widen it.

The keeper (`bridge/keeper.rs`) asks every 5 minutes. A no, or a credential the server refuses, ends it at once; no answer ends it once the
last yes is 30 minutes old; signing out or changing server ends it at once; and without a yes nothing starts. The answer is in the
process's memory only: `experimental.vrchat_bridge` in the settings file can only turn it off for someone who has it. For everybody else
Settings shows no section, and nothing listens.

## Turning it on

Settings, Experimental, VRChat bridge. It then shows Stop, whether a client is connected, a row per agent, the address and token for any other client, and the recent actions.

**One click per agent.** Claude Code and Codex each have a row showing what that client itself answers, from `bridge/clients.rs`: not installed on this computer (nothing offered), not added (**Add to Claude Code**, **Add to Codex**), added (Remove), added with an old address or token (**Update**, Remove), added and it says it cannot connect, with its words. A new session of that client has the bridge with nothing else to do; one already open has to be restarted, and the row says so.

- **Claude Code:** `claude mcp add --scope user --transport http orbiters-vrchat <address> --header "Authorization: Bearer <token>"`, read back with `claude mcp get`, whose status comes from really connecting.
- **Codex:** one block at the end of `~/.codex/config.toml`, read back with `codex mcp get --json`: `[mcp_servers.orbiters-vrchat]`, `url`, and `http_headers = { Authorization = "Bearer <token>" }`. The exception to using a client's own command, for two measured reasons: `codex mcp add` has no flag for a header, only `--bearer-token-env-var`, and an environment variable never reaches a terminal or editor already open, so the server would fail in those with nothing to say why; and on Enzo's file (Codex 0.166.0) `codex mcp add` rewrote all of its servers, dropping a marker comment another tool keeps there, every `enabled = true` and `args = []`, and reordering environment variables. Remove takes the block away, byte for byte, and any `codex mcp` command of the person's own may rewrite the file as it pleases: the entry survives that, in the form Codex writes it.
- **The token** lands only in those two files of this user's profile, where each client keeps its other servers' credentials (`icacls`: this user, administrators, system; Codex's also its own sandbox's group). No shell runs, so it is in no history; it is on `claude mcp add`'s command line for the half second that runs, visible only to this user's programs and administrators.
- **New token, moved port:** every time the bridge opens and after New token, an entry with old values is rewritten by the client's own commands, and Settings and the log say so. A session already open keeps what it read until it restarts.

Needed as well: signed in to VRChat in Settings (private instances are made with that session); the same account in the game; OSC on in VRChat's Action Menu, Options, OSC; **an avatar with N4N0's capture kit** (else a recording has no keyframes: `get_state`'s `avatar.capture_kit` says so as soon as the game has arrived, and `take_3d_picture` refuses).

## The tools

| Tool | What for |
| --- | --- |
| `get_state` | Everything readable, and `next`: what to do now. |
| `find_worlds` | World ids: search, visited, favorites, trending (VRChat's `sort=heat`, its "Hot" row), popular, active, and what VRChat highlights: `spotlight_pc` and `spotlight_cross_platform`, its two Spotlight rows (`tag=admin_spotlight_pc` / `admin_spotlight_xplat`, `sort=updated`, as VRChat's own `GET /config` `dynamicWorldRows` name them, read 2026-11-15); the other rows of its Worlds menu from the same list: `community_labs` (`sort=labs`), `random` (`shuffle`), `avatar_worlds` (`heat`, `tag=author_tag_avatar`), `games` (`heat`, `tag=author_tag_game`), `updated_recently` (`updated`), `new` (`publication`), `classics` (`trending`); and `playlists`, the world favourite groups of the VRChat account signed in in the app (`GET /favorite/groups?type=world`, then `/worlds/favorites?tag=<group>`). 24 worlds a page, `more` and `next_page`; VRChat's lists are read 21 pages deep (504 worlds), past that `bad_arguments`. |
| `go_to_world` | A new private instance of a world. A side session's running VRChat travels there in the game (its link pipe is handed the instance's link; VRChat shows the world's page and the agent clicks Join): one start of VRChat for a whole tour. The main session, a side session with nothing running, a pipe served by another VRChat, or `relaunch: true` start VRChat in it (closing the last one). |
| `close_vrchat` | Closes the game the bridge started. Reconstructions then run. |
| `wait_for` | Waits for arrival, closing, a saved picture, reconstructions, the end of a pause. |
| `screenshot` | VRChat's window as a JPEG, with timestamp, sizes and scale. |
| `walk` | Over OSC, at most 10 s a call. Says the speed VRChat reported. |
| `turn` | Over OSC, about so many degrees at 165°/s (measured), and says it is approximate. |
| `press` | One input for a short time: `jump`, `quick_menu`, axes over OSC; a short list of keys (movement, `esc`, `r`, and for worlds' own screens and games `tab`, `e`, `f`, `q`, `0` to `9`). |
| `mouse` | Moves the pointer or clicks in the window, or looks (relative). Coordinates are for the size the window started with: a window someone resized is put back first (never activated, never brought to the front) and the click refused with `window_resized`, so that the agent looks again. `screenshot` puts it back too (`window_restored`), while the person is away. |
| `focus_game` | Asks Windows to bring the game forward: Windows may say no. |
| `wear_avatar` | Changes to an avatar by its `avtr_` id, over OSC, and waits for the change. Only when the person asked. Says `changed` false when VRChat did not take it, and `avatar.capture_kit`. |
| `take_3d_picture` | 1, 4 or 8 seconds, then the session and its reconstruction's state. |
| `aim_camera` | Moves VRChat's open camera itself over `/usercamera/Pose` (forward, right, up from where it is and the way it looks, turn, pitch; at most 25 m a call), taken as the pose the next picture starts from. Vantage without walking or menus. |
| `map_plan` | This world's finished 3D pictures (bridge pictures whose reconstruction is done; kept per picture in `bridge-maps/` of the app folder from the first read, so `free_picture_files everything` does not lose them) as a plan from above: voxels of 0.25 m solid where two splats or more fell, open where rays from the flight to what it saw crossed, unknown elsewhere; per 0.5 m cell the ground grown from where the avatar stood, and blocked/floor/open/unknown. Answers the size, a verdict on how many pictures the world needs (one for a very simple world), the earlier centres, the camera and the avatar (from the last picture's `skeleton.bin`, then the bridge's own walks and turns), up to three suggestions 10 m or more from every centre with `aim_camera` calls and a walk plan, and a PNG of the plan. Pictures over 150 m from the newest are another part of the world (teleports) and left out. Frame and its proof: `bridge/map.rs`. |
| `map_check` | The same map on a camera point, an `aim` from the camera, a `walk` from the avatar or a `walk_to`: free, near_solid, solid, unknown or outside; how far a walk is clear and what stops it. `aim_camera` and `walk` add this as a `map` note (a warning, never a refusal). |
| `orbiters_environment` | Reads or switches the app's Orbiters environment (Settings' Development environment switch: account, labels, publishing, update feed, links), saying what changes; refused while labels wait; logged. `get_state.orbiters` always says it. |
| `publish_3d_picture` | Publishes a reconstructed picture the bridge took to the active environment, as Photos & captures' share does (public, or private: an upload kept on the account and listed nowhere; the same upload records), answering the item's id and link. One at a time in the order asked; a 429 from Orbiters holds the queue for its `Retry-After` (answered `rate_limited` with `retry_after_s`) and it goes on by itself, never sending again what Orbiters accepted. One public capture per account and world: a world's others go private, the best public last. |
| `pictures` | Every picture the bridge took, kept across restarts in `bridge-journal.json` (and those found in the sessions folder: 3D pictures in a strict private instance of a side session's own account, never the person's), with size on disk, reconstruction and publishing. |
| `free_picture_files` | `capture_files`: keyframes, depth and leftover runs once reconstructed (`cleanup::drop_recording`: what a share reads stays); `everything`: the whole folder once published (or nothing can be). `get_state.disks` and `disk_warning` say the room left. |
| `world_labels` | N4N0's world labels (the fixed list of contracts `Schemas/world-feedback-labels.json`, with the reason `ai-world` needs), a world's labels with their counts and the account's own, and the Orbiters account and server labels go under. Default: the session's world. |
| `label_world` | Gives (`add`) or takes back (`remove`) labels on a world a bridge session arrived in (kept across restarts), through the very queue a person's Label it on the worlds page uses (`Controller::world_feedback_ask`): same rules, account and limits (120 changes an hour, one per world). Anything outside the list is refused and nothing is sent; no Orbiters account, `orbiters_not_signed_in`. The descriptions say labels are a published, person-facing judgement, given only for what the agent saw. |
| `open_session` | A VRChat of its own in a profile (`--profile=N`, N ≥ 1), beside any other VRChat: its `session` id, its account or `needs_sign_in`. |
| `list_sessions` | The sessions, their profiles, accounts, worlds, who drives each and how long it has been idle (ids shortened). |
| `close_session` | Closes a session's VRChat and ends the session. |

Every VRChat tool takes `session`; without it, it acts on the main session as before.

### Sessions: several VRChats at once (2026-11-15)

A **session** is one VRChat the bridge started, bound to a VRChat profile, driven by one agent. `bridge/sessions.rs` holds the rules,
`controller/bridge_side.rs` the real side sessions, `lab.rs`'s `LabOpener` the fake ones the tests drive.

- **main**: what the bridge always had, and what tools without `session` act on. The VRChat the app itself follows (default OSC ports
  9000/9001, the newest log, the capture), so only while the person runs no VRChat.
- **side sessions** (`open_session`): started by VRChat's own `launch.exe` with `--no-vr --profile=N --osc=<9000+10N>:127.0.0.1:<9001+10N>`
  (Steam's `-applaunch` starts a game once). Each has its own OSC ports, its own log (the one whose arguments name its `--osc`), its own
  process (the one VRChat that appeared after its launch: launches are one at a time) and window, its account (from its log:
  `post login checks for <name> (usr_…)`, or `needs_sign_in` from `No cached credentials found`), its own pause and held inputs, and a lease.

What is one per computer, and what became of it:

| Shared thing | Now |
| --- | --- |
| The app's VRChat (process, log, OSC, OSCQuery) | Follows the person's VRChat (or main), never a side session: the process watcher skips side sessions' pids, the log watcher their logs, OSCQuery discovery takes only a `VRChat-Client` whose OSC port is 9000, and their datagrams to the app's OSC port (they find it over mDNS like any VRChat) are dropped by the owning process of their source port (`GetExtendedUdpTable`). |
| OSC ports | Per session (`--osc`); main and the person keep 9000/9001. |
| Keyboard and mouse focus | One window at a time: keys, clicks and `focus_game` take **turns** (`FocusTurn`, first asked first served, a turn kept while used and passed on 8 s after its last use: `focus_busy` with `retry_after_s`). A session brings its own window forward on its turn, only with the person away from the computer, and never when the foreground is a VRChat the bridge did not start (`person_vrchat_has_focus`). OSC (walk, turn, OSC inputs) and screenshots (Windows Graphics Capture of the window) need no focus and run in parallel. |
| Window capture | Per window. |
| VRChat's log | Per session (its own `output_log`, found by its `--osc` argument). |
| The person wins | Their input in a side session's window pauses that session only; their input anywhere pauses every key and click (and main, as before); the hotkey and Settings' Pause hold everything. |
| 3D pictures (camera, Spout `VRCSender1`, capture worker, OSC camera flight) | The app's, **lent** to one side session for its recording (`Controller::lend_vrchat`: process and log watchers, OSC target, OSCQuery, datagrams and the session's last camera pose all point at it), then given back. One picture at a time on the computer (`picture_busy`), and none while a VRChat the bridge does not run as a side session is the app's (`app_in_use`: the person's own, or main). |
| Pictures and account | In Orbiters - N4N0's sessions folder like every capture, published under the Orbiters account signed in in the app; each session's `get_state` lists its own. |
| Instances | Main: made through VRChat's API by the VRChat account signed in in the app, as before. A side session: VRChat refuses another account in that one (`Failed to join instance … due to 'You are not allowed to travel to that location'`, measured 2026-11-15), so the session names a new strict private instance of its own account, as VRChat's client does (`<n>~private(<its usr>)~region(<r>)~nonce(<uuid>)`, `sessions::own_private_instance`), joined at once in the same test. |
| Easy Anti-Cheat | Shared by all VRChats (one service process, one driver). A launch waits until it may (`Eac::may_launch`, read every 2 s): beside a running VRChat at once; with none, until service and driver are stopped (else `eac_busy`). Never while the driver service's registry key has `DeleteFlag` 1 (`eac_stuck`: marked for deletion, no VRChat can start until Windows restarts; 2026-11-14 22:13 and 2026-11-15 07:04). Easy Anti-Cheat's `anticheatlauncher.log` outcome is read by its code (171 logs on the owner's PC, 2026-11-09 to 15): `301, 'Easy Anti-Cheat successfully loaded in-game'` is the game starting (it comes about 1.5 s before VRChat.exe; taken for a failure on 2026-11-15 12:25, it disowned the VRChat just started), `206` a failure (`CreateService failed with 1072`, `EAC unload timeout`), any other code no failure by itself. A start waits for its VRChat 30 s, and 30 s more unless the launcher said 206. With none, its launcher (`start_protected_game.exe`/`launch.exe` with the session's `--osc`) is ended, the VRChat is looked for once more, and only then `start_failed` comes, with the launcher's words, what was checked and `retry_after_s` 90; a failed `open_session` releases its OSC listener. A VRChat of a start given up on that comes up within 5 minutes is adopted by its session (`Game::adopt`: its log's command line has the session's `--profile` and `--osc`, it was created after the launch, and exactly one VRChat that no other session or main has started in the minute before that log). `open_session` on a profile whose VRChat runs adopts it by the same rule (a start answered failed, or one left by an earlier run of the app) instead of answering `profile_in_use`; a VRChat of the default profile, or without the session's ports, is never adopted. A side VRChat that never signed in is not closed before 90 s (`too_young_to_close`). |
| VRChat's link pipe (`VRChatURLLaunchPipe`) | Only the first VRChat started gets it (`All pipe instances are busy` in the others). A `vrchat://` link the person clicks goes to whichever VRChat has it: if a side session started first, its world page opens there, not in theirs. |
| Which VRChat the app follows (2026-11-15, `controller/vrchats.rs`) | The app knows every VRChat on the computer (the roster): each process with its own log (made as it started), that log's `--profile`/`--osc`, account, world, desktop or VR, OSCQuery port and camera. One is its **target**: the person's own by default (the one on VRChat's default ports first), or the one the person picks on the Capture page (one click; the status column at the bottom left counts them). The process and log watchers, OSC (input port, tree, the datagrams taken: only the target's process's) and the capture follow the target; what another VRChat says of its camera is kept for it. A VRChat the bridge started for a side session (its `--profile` ≥ 1 and that profile's `--osc`) that no session holds is never the person's. `list_sessions` and `get_state` say the same as the Capture page (`vrchats`: profile, account, world, mode, camera, `driven_by`, `pictures_taken_here`, `chosen_by_person`). When the person chose a session's VRChat, that session's `take_3d_picture` answers `app_in_use` and its input waits (`paused_by_person`) while they record there. Before a recording, every other VRChat whose camera streams over Spout has its Spout Stream switched off until it ends: all of VRChat's stream cameras are named `VRCSender1`, and its Spout plug-in (KlakSpout, an older Spout SDK) keeps one info block per name, the last writer's. |
| Limit | `experimental.vrchat_bridge_sessions` in the settings (default 2, at most 4: main and side sessions together), and 4 GB of free memory for a new one: `too_many_sessions`, `low_memory`. |

Ownership: `open_session` returns an id with an unguessable part; every call with it renews its lease; `list_sessions` never shows whole ids;
a session without a call for 10 minutes may be taken over (`open_session` with `take_over`: same VRChat, new id, the old id refused with
`session_taken_over`). The person's own VRChat, and any VRChat the bridge did not start, is never driven, focused away from, or closed.

### VR sessions: the tour goes on while the person works (2026-11-15)

Measured on profile 1's desktop VRChat with the owner at the computer (report `vr-feedback-1111/done7-nofocus.md`): VRChat in desktop
mode takes keys and clicks only from raw input of the foreground window. Window messages, even with a faked activation, do nothing.
Its camera cannot be opened over OSC: `/usercamera/Mode` is not even stored while the camera is closed, and `/dolly/Play` does nothing.
The camera also closes at every world change, and the Join of a world page needs a click. So a desktop session needs the person away
for every world. Window captures and OSC movement work in the background.

`open_session {profile, vr: true}` makes a **VR session**:

- It takes a turn on the developer VR sandbox (`tools/vr-sandbox`, `sandbox.py acquire --keep-app`): SteamVR with the emulated headset
  and controllers, the person's installed app left running. It only exists where `experimental.vrchat_bridge_vr_sandbox` names that
  folder; otherwise `vr_unavailable`. The person's own VR refuses it (`vr_busy`): their headset on Virtual Desktop, their SteamVR, or
  their VRChat.
- Then VRChat starts as for any side session, without `--no-vr`, so it takes SteamVR.
- `screenshot` is the left eye (`sandbox.py call capture`).
- `mouse move|click` aims the right controller's laser through a pixel of that picture (sandbox action `point`: VRChat's laser is
  64.5° below the controller's -Z) and pulls the trigger.
- `open_camera` is the Action Menu route (`camera`) plus OSC Stream.
- `press` of keys and `focus_game` answer `vr_session`.
- None of this is the person's keyboard or mouse, so `paused_by_person` does not apply. The hotkey and Pause still do.
- The turn is renewed every 2 minutes while the session lives. `close_session` gives it back.

The app leaves the sandbox's SteamVR alone (`steamvr_start::sandbox_runs`: the external driver named `orbiters_sandbox` in
`openvrpaths.vrpath`): no overlay, no sound devices, no SteamVR start choice. Measured cost of a VR VRChat (1080x1200 eyes, 72 Hz) beside
the desktop one: GPU 35-41 %, 6.2 GB of graphics memory and 82 W in all, against 38 %, 2.9 GB and 37 W before.

The resource `bridge://guide` and the prompt `world_tour` explain the loop to an agent, including when to give up on a world. Every refusal has a
`code`, a `message` and a `next` (and `retry_after_s` when that is the answer).

### The Unity projects tools

The same server, behind the same gate, address and token, also does what the **Unity projects** tab does
(`crates/orbiters-desktop/src/bridge/projects.rs`). They run in the tray service and call what the tab's buttons call
(`orbiters_projects`: the VCC environment through vrc-get, the projects found on the disks, Unity's editors, the Library
move, the templates), so they work with the window closed and behave as the tab does; each change bumps the snapshot's
`projects_changed`, and the tab looks again. Their guide is the resource `bridge://unity-projects`.

| Tool | What for |
| --- | --- |
| `unity_list_projects` | The projects on this computer: name, path, Unity version, kind, listed or only found, hidden, avatars and worlds (blueprint ids), Unity open, size. `refresh` looks again (a job). |
| `unity_get_project` | One project: installed packages and versions, updates and held updates with why, Update all's plan, Unity's editor, disk health, Git, Blender files, previews, worlds' captures and uploads, missing packages. |
| `unity_find_packages` | What the enabled repositories offer, with versions (pre-releases on request) and the installed one. |
| `unity_change_packages` | `install` (a version or the newest; also changes a version), `remove`, `update`, `update_all`, `restore` (Resolve), `apply_plan` (a conflict's plan), `solve` (a plan, nothing applied). A job. |
| `unity_missing_packages` | After `packages_not_listed`: `lookup` finds the repositories that have them, `add` adds them and runs the change again. |
| `unity_repositories` | `list`, `add` (URL, headers), `remove`, `enable`, `disable`, `refresh` (download again), `suggest` (Orbiters' catalog). |
| `unity_editor` | `status`, `open` (Open in Unity), `show`, `save_and_exit`, `kill` (needs `confirm`). |
| `unity_manage_project` | `add` to the list, `remove_from_list` (exact listed path and `confirm`; files stay), `hide`, `unhide`, `reveal`, `open_hub`, `open_blender`. |
| `unity_list_templates` | Templates, their Unity version and whether it is installed, the default folder. |
| `unity_create_project` | A project from a template, listed in VCC and ALCOM. A job. |
| `unity_duplicate_project` | Duplicate with exact packages, Library optional; `check_only` gives bytes needed and free. A job. |
| `unity_storage` | `check` the disk, `plan` and `repair` the Library move, `measure` the size. Jobs. |
| `unity_add_orbiters` | `mixer`, `toolkit` (and open in Unity), `my_avatar`, `n4n0` on one of the project's avatars. A job. |
| `unity_world` | A world project's world: `status`, `add_pictures`, `set_preview` (a capture recorded there). |
| `unity_get_job`, `unity_cancel_job` | Follow a job (`wait_s`), stop a copy, a Library move before its switch, a scan or a size. |

Rules: one package or project change at a time (as the tab), the Library move, a scan and a size each in a lane of their
own; a pause held from the hotkey or Settings refuses every change (reading goes on), the person's own input refuses only
what opens a window (`open`, `show`, `reveal`, `open_hub`, `open_blender`); Stop cancels what can be cancelled. Nothing
deletes a project's files: the tab has no such action, and neither do the tools. Not covered: the 3D preview's own view
(drawn on the window's GPU; the tools list the previews), and "Change the picture on VRChat" of a row's menu (a write to
the VRChat account, which the bridge does not make).

## The protocol

Streamable HTTP on `127.0.0.1`, one POST endpoint answering JSON, revision `2026-07-28` (stateless, `server/discover`, the `Mcp-Method` and `Mcp-Name`
headers, `resultType`) and, for clients that start with `initialize`, `2025-11-25`, `2025-06-18` and `2025-03-26` (no session is minted; `GET` and
`DELETE` get 405). Hand-written on the standard library's sockets, as the tray service's other listeners: the official Rust SDK, rmcp, is
sound and Apache-2.0, but brings tokio, hyper and axum into a process that has none of them, for one route that answers JSON, and
the part that must be this service's own (the checks before anything else) is most of the route. No new crate came with this.

## The rules, and where they live

- **Only loopback, a token, and its own name.** `127.0.0.1` only. A random token per install, kept with DPAPI (`vrchat-bridge.dat`). `Host` must be
  this listener's, against DNS rebinding; an `Origin` other than its own gets 403; `application/json` only; no CORS header ever. `bridge/mcp.rs`.
- **Nothing listens unless both allow.** `bridge/keeper.rs`. It also stays closed when the person cannot be heard or the guard cannot start.
- **Only the game it started.** A VRChat the person started is theirs: not moved, not looked at, not closed. `tools.rs`, `ours`.
- **Only private instances of the signed-in account.** `go_to_world` takes a world id and a region, nothing that names an instance. The answer of VRChat's
  API is checked, and so is what the game's log says: `private_instance_of` accepts `~private(<me>)` only, not invite+, friends, friends+, group, public or
  anyone else's. Anywhere else, movement, input, pictures and screenshots are refused and the agent is told. A start that joins any other instance than the
  one made for it, and a VRChat that moves on from its private instance to one that is not its own private one, is closed by the bridge at once
  (`landed_elsewhere`, with the instance, its kind and how many people were there): it never stays. `tools.rs`, `follow_game`.
- **Keyboard and mouse only into the foreground game window.** Checked right before `SendInput`, and during a held key. Mouse positions must be inside the
  window. OSC needs no focus, and is the first choice. The bridge asks Windows to bring the game forward on arrival and through `focus_game`, only while the person is not paused-for (no
  input of theirs for 30 s): an unattended run cannot open the camera otherwise.
- **Screenshots are of that window only.** Windows Graphics Capture for that handle in a short-lived process (the service loads no GPU driver), `PrintWindow` for
  the same handle if that fails. Never the desktop.
- **Nothing types or speaks.** The inputs are an allow-list without `Y`, `V`, Enter, Backspace, the F keys, Ctrl, Alt, `/input/Voice` or `/chatbox/*`; its letters and digits (movement, `e`, `f`, `q`, `0` to `9`, `tab` for worlds' own screens) type only into a text field already open, and nothing on the list opens one. A click can reach anything on the game's
  menus, invites and avatars included: the guide and the tool's description forbid those, and nothing can enforce it. An avatar the person named goes through `wear_avatar`, never the menus.
- **The person always wins.** Any real key or mouse movement (Raw Input, whichever window has the focus, told from our own by having a device;
  `GetLastInputInfo` as a second opinion) pauses the agent and releases input; it goes on 30 s after the last of it. `Ctrl+Alt+F12` or Pause pause it until resumed. Stop
  turns the bridge off, ends what runs (a recording too), closes the port, and leaves the game as it is, no longer the bridge's. `bridge/person.rs`.
- **Nothing stays down.** A press lives in one call, at most 10 s, and comes up at its end, when the client disconnects, the person takes over, the instance stops
  being private or the focus goes. A sweep brings up anything past its time. If the app itself dies, `--bridge-guard`, told what is down on its standard input,
  brings it up when that input ends. `bridge/input.rs`, `bridge/guard.rs`.
- **Restraint with VRChat's API.** The session's own limiter (one request a second per family, 429 backoff across accounts), and at most one world every 30 s.
- **The GPU rules stand.** Reconstructions follow Settings' "Reconstruct while VRChat runs" as ever (the default, Graphics card, trains beside the game and gives way to its frames). Set to wait for VRChat, they do not slip into the moments the game is closed between two worlds.

What it uses: startup arguments, OSC, ordinary window input, window capture, the game's log, and VRChat's API with the session the app already holds.

## How it starts VRChat

`steam.exe -applaunch 438100 --no-vr -screen-fullscreen 0 -screen-width 1280 -screen-height 720 [--profile=N] vrchat://launch?ref=orbiters.cc&id=<world>:<instance>`, else, without Steam, VRChat's own
`launch.exe` (from the `vrchat://` link it registers) with the same arguments. Never `VRChat.exe` itself: that goes around the launcher Easy Anti-Cheat starts from. `-applaunch`
because, unlike a `steam://` link, it asks nothing. No `--osc`: VRChat's defaults are what the app's OSC finds through OSCQuery. In the main session changing worlds restarts the
game. A side session travels in its running game instead: the instance's `vrchat://launch` link written to VRChat's link pipe (`\\.\pipe\VRChatURLLaunchPipe`,
what `launch.exe` does for a running game), only when that session's VRChat is the pipe's server (`GetNamedPipeServerProcessId`; the first VRChat started
serves it), never to the person's. VRChat then shows the world's page with a Join button (measured 2026-11-13 in the sandbox), which the agent clicks.
Each start of VRChat makes Easy Anti-Cheat delete and install its driver service again (System log event 7045 about 40 times in two hours on the
2026-11-15 tour), and twice a long run of such starts ended with the driver service stuck "marked for deletion" until Windows restarted.

## Testing

Nothing here may touch the person playing on the computer. `bridge/lab.rs` is a fake VRChat: a real window off every screen and never activated, found, captured and closed through the
real calls; a real UDP listener for OSC; a log in VRChat's format read back by the real `LogWatcher`; and a stand-in for `features/mine`, asked through the real `Api`. It records keyboard
and mouse input instead of sending it, and "having the foreground" is a switch for the same reason (off, the real check on the real window says no).

- `cargo test -p orbiters-desktop bridge::`: the rules and the whole loop over the real transport (`bridge/e2e.rs`).
- `node scripts/bridge-lab.mjs <orbiters-desktop.exe> <folder with @modelcontextprotocol/sdk>`: the same from MCP's official client against `--bridge-lab`, including killing the
  process mid-walk and hearing the guard release it.

## Running it live

For an agent that has to look at the real game (its brief must allow it). VRChat's folder lock first, as the shared rules say: `mkdir C:/Users/Enzo/.orbiters-build/sota/bench/vrchat.lock`, name and time in `info`, `rm -rf` it at the end. If `tasklist` shows VRChat and you hold no lock, it is Enzo's: wait.

```bash
# 1. Your build as the tray service, beside the install's helpers. Same settings, same account, same port and token.
R=C:/Users/Enzo/.orbiters-build/sota/bridge-live-run; I="C:/Users/Enzo/AppData/Local/Programs/Orbiters - N4N0"
mkdir -p $R; for f in "$I"/*; do b=$(basename "$f"); [ "$b" = install.json ] || cp -n "$f" $R/; done
cp C:/Users/Enzo/.orbiters-build/sota/target-desktop/release/orbiters-desktop.exe $R/   # after deskcargo.sh build --release
"$I/orbiters-desktop.exe" --quit                # a clean stop of Enzo's service: only one instance runs
(cd $R && nohup ./orbiters-desktop.exe --hidden > service.log 2>&1 &)   # this folder: Windows' firewall already allows the program at this path

# 2. Drive it. By hand, over the real transport; the token is read from the app's file and never printed:
python scripts/bridge-live.py get_state
python scripts/bridge-live.py find_worlds '{"source":"trending"}'
python scripts/bridge-live.py go_to_world '{"world_id":"wrld_..."}'     # arrives when the world has loaded: 33 s from the cache
python scripts/bridge-live.py screenshot '{}' shot.jpg                  # LOOK at it, every time
python scripts/bridge-live.py press '{"input":"esc"}'
python scripts/bridge-live.py mouse '{"action":"click","x":640,"y":620}'
python scripts/bridge-live.py close_vrchat
#    Or as an agent would: Settings' row adds it for every new session.
claude -p "Use the orbiters-vrchat MCP server. Read bridge://guide. ..." --allowedTools "mcp__orbiters-vrchat" --model sonnet

# 3. Put it back.
$R/orbiters-desktop.exe --quit
"$I/orbiters-desktop.exe" --hidden &    # and without --hidden if the window was open
rm -rf C:/Users/Enzo/.orbiters-build/sota/bench/vrchat.lock
tasklist | grep -iE 'vrchat|orbiters'   # nothing of yours left
```

Keep the service in that folder, and **do not run it with `ORBITERS_NO_NETWORK_LISTENING`:** that switch is for test programs, and it turns off the mDNS that finds VRChat's OSCQuery. Tried on 2026-11-06: `osc_answers` false, camera and capture kit unknown, so no picture. A program at a new path makes Windows' firewall ask, in a prompt that takes the foreground from the game until a person answers it (`focus_game` then says Windows refused). A build that needs this checks with the `screenshot`'s `foreground`, and `Get-NetFirewallApplicationFilter` lists the rules.

The agents have no tool that changes avatars, on purpose. `get_state` says `avatar.capture_kit`; a test with an avatar that has no kit needs the person to change it, or their word that the test side may, over VRChat's `/avatar/change`, and put it back.

`cargo test replay_a_session -- --ignored --nocapture` (`SESSION`, `AT`, `HIPS`, `EYE`) prints what a real session's depth frames said along a line, frame by frame: how to ask why a flight went somewhere.

A screenshot takes 0.3 to 0.5 s with the game at 60 to 80 frames a second, so it is fine to look often. For an overlay or a menu of the app's own in VRChat: a picture shows the game's window only, and what a desktop VRChat draws. Anything of yours that injects input or moves the mouse counts as the person and pauses the agent for 30 s. **Look before every click:** see the shop below.

## What the real game showed (2026-11-06, Windows, VRChat in desktop mode, Easy Anti-Cheat running)

- **Steam passes everything.** `steam.exe -applaunch 438100` gives the game `--no-vr -screen-fullscreen 0 -screen-width 1280 -screen-height 720` and the `vrchat://launch` link, and asks nothing. Enzo's own launch option comes after them. A window of exactly 1280 × 720 client pixels, into the private instance, not Home.
- **`POST /instances`** answers as the specification says: `~private(<me>)~region(eu)`, and the log's join matches. 15 s from start to `Joining`, 33 s to a loaded world in the cache.
- **Foreground:** yes on arrival, each time.
- **`WM_CLOSE`:** 3.2 to 3.6 s, asks nothing. Easy Anti-Cheat's service stays a moment.
- **Screenshots:** Windows Graphics Capture works on the game, 0.3 to 0.5 s each, and shows the present. No yellow border in the picture.
- **`SendInput` works:** Esc by scan code, pointer exact to the pixel. **Menu clicks need the pointer to rest first:** a press 80 ms after arriving was usually ignored, one after about 500 ms held each time, so `click` rests that long.
- **Windows reports our own input without a device** (`hDevice` null), as assumed, but the person's listener woke on it before the sender had said it was ours: every click paused the agent for 30 s. It says so before sending now.
- **Walking over OSC works without the focus, only with no menu and no camera open:** `speed_seen` 2.0 m/s, about 4 m in 2 s. With the camera or a menu open `/input/*` and the keyboard do nothing at all (`speed_seen` 0). So: walk first, open the camera last. Esc closes the camera too.
- **Turning:** about 165°/s by holding `LookRight`, not 90.
- **The camera opens in desktop mode:** Esc, the camera icon in the menu's bottom row, Stream Camera. The panel stays on the screen.
- **The flight flies in desktop mode:** VRChat takes `/usercamera/Pose` at 90 Hz, its viewfinder went with it, Spout streamed at 41 frames a second (`VRCSender1`, 1920 × 1080) and the app turned Spout on and off by itself.
- **The menu has a shop.** A banner on the first page opens a page with a Purchase button, and a click that landed on the wrong page went there. No purchase happened, and Esc closed it. Nothing but the agent's own looking can guard this: the guide and the tool's description say so.

**With a kit avatar on** (three more runs that day, from `claude -p`; Toolkit 0.4.11, so no hide layer):

- **The whole loop works:** a trending world, a private instance, 8 s of World picture, 1,104 frames, 561 keyframes (75 colour, 486 rig faces), 97 depth frames, and 852,000 splats in 37 s beside the game at 38.3 dB on views it never saw, in "everyone is asleep except you". Turned views hold up. The recording takes about 9 s.
- **Software removal works** on the real avatar: 15 % of the pixels of 246 of 561 views left out, 16 Gaussians left in the volume. Small dark marks stay on the floor at the feet.
- **The person's items are in the picture.** VRChat's items follow their owner (`[VRCItems] Item prop_… spawned by usr_…`): Enzo's Ghost Companion floated 0.7 m from the hips. What floats free inside the wearer's envelope now goes with them (`orbiters_reconstruct::wearer`). One following further away than that stays.
- **Closed doors and glass can write no depth,** so depth reads sky through them. A flight left the building through one, 7 m away, and the frames trained the city and a blue-lit door into the corridor. Sky now vouches only up to the surfaces around it. That fix is proved by replaying that flight's depth and by a later run elsewhere that stayed inside, not by flying at that door again.
- **A wearer in the depth closed a corridor behind the camera.** One who leaves the picture afterwards is no obstacle now, and the camera may pass through them: such a view has no pixel left, and is not held out, as it once ended the whole reconstruction.
- **In a narrow place** the rings find no way (`no ring 0 m up: depth shows no way`), and the flight spends its time on hops.
- **Reconstructions run beside the game with the default setting,** at low priority, and may be done before the game closes. One that fails says `failed` and why in `get_state`.
- **The person's own input pauses a running game's agent:** seen once, and it went on 30 s later. The hotkey and Stop have not been tried on the real game.

## Trying it on the real game

With VRChat closed, signed in to VRChat in Settings, an avatar with the capture kit on, turn the bridge on in Settings, press Add to Claude Code, start a new Claude Code session and paste:

> Use the orbiters-vrchat MCP server. Read bridge://guide. Then go to two worlds of your choice from my favorites, take one 4 second 3D picture in each, close VRChat, wait for the
> reconstructions, and tell me what happened at each step, including what you had to try twice.

What should happen: VRChat opens in a window at 1280 × 720, in a private instance (the instance's name shows `~private(` in Settings' log and in `get_state`). The agent walks somewhere, opens the camera through Esc and the menu, the camera flies, and N4N0 records. The game restarts into the second world. At the end it closes, and the reconstructions finish (with "After VRChat" in Settings, they start then).

What to look at: did it stay out of your way until you touched something? Touch the mouse: it should pause at once, and go on 30 s later. Press `Ctrl+Alt+F12`: paused until pressed again. Press Stop: the
avatar stops at once and the port closes. If a step fails, the agent's text and Settings' Recent actions say which: send those.
