# Working on Orbiters - N4N0

- **Windows and macOS (Apple silicon only) are both shipped.** Whatever VRChat gives on the same computer (Spout, SteamVR,
  its log) has nothing to read on a Mac (`orbiters_core::osc::VRCHAT_RUNS_HERE`), and a new feature that needs it is hidden
  there; one that goes over OSC talks with the VRChat on the network (`orbiters_core::lan`). Native code goes behind
  `cfg(target_os = "macos")` beside the `cfg(windows)` path. `cargo xtask` on a Mac makes the Mac's release:
  `target/dist/<version>/Orbiters-N4N0-<version>-macos-arm64.dmg` is the build to give (see `docs/releasing.md`).
  VRChat itself does run on a Mac, one game at a time on the profile chosen on Home (`vrchat_launch::SEVERAL_HERE`; every start of
  Home's carries it, a restart included, with no argument saved in the engine; an account is learnt only from that launch's own log;
  on a computer with several profiles, here or on Windows, no start carries a private place, an invite or an instance made for an
  account: VRChat starts without it, and it is delivered once that very VRChat's own log proves the account, `vrchat_launch::way`,
  the one rule of every start and delivery; with one profile it goes as it always did, and no log is asked; "Several VRChat accounts
  on one computer" below; `docs/vrchat-bridge.md`),
  started by N4N0 through
  Obelisk's engine without its window: `obelisk-cli`, built
  from the fork github.com/Orbiters-cc/Obelisk (branch `cli`, GPL-3.0, its `docs/CLI.md` is the contract), a separate
  program N4N0 downloads from that fork's releases and runs (`orbiters_core::obelisk`, `controller/obelisk_cli.rs`).
  Nothing of N4N0 goes into the fork, and none of the fork's code into N4N0.
- **Every change keeps `cargo check` passing for the Mac** (main did not compile there from 0ee59341 to 2026-11-25; the tag 0.3.10 does
  not). From Windows: `scripts/linux/check.sh mac` (`setup` first; it type-checks without a Mac); what is Windows' alone stays behind `cfg(windows)`.
- **The Orbiters menu around VRChat's is on a Mac too** (`overlay/orbit/desktop_mac`, `docs/orbit-overlays.md`, "On a Mac"): the same
  parts and detectors as Windows' `desktop.rs`, which is not to be edited for a Mac (its tests cannot run here: share through
  `desk_shared.rs`, and check with `cargo check --target x86_64-pc-windows-gnu -p orbiters-desktop --tests`). Ordinary panels of ours over
  the engine's window, and nothing else: no event tap, no Accessibility, nothing sent to the game or `wsd`, no pointer warp while
  the game holds it, and **never windows that together cover all of the game's** (macOS stops showing it: `panel::uncovering`).
  **No key points** with obelisk-cli 0.1.15 or later: its engine lets the mouse go where VRChat does (the Quick Menu) and says so on a
  read-only channel (`desktop_mac/channel.rs`), and the ring round the wheel, which holds the mouse, is picked by pushing past its rim
  (`gesture.rs`: its thresholds keep ordinary use of the wheel from ever picking ours; never let a click confirm). The game keeps its
  keyboard but while a text field of ours is up. Left Option is for an engine from before only. Screen Recording is the one
  permission (Input Monitoring only if macOS hides the keys without it). Hidden there: 3D pictures, the camera's column,
  translation, the corner's cards and mismatch buttons. `tools/mac-overlay/check.py` runs it over the engine's real window server
  with no game, only while nobody uses the Mac.
- **The VRChat bridge on a Mac observes and travels only** (`docs/vrchat-bridge.md`, "On a Mac"). What a computer's bridge can
  do is `bridge/caps.rs`, never `cfg` in shared logic: Windows has every capability (keep it so: its tools, guide and tests are
  unchanged), and a Mac's tool list, guide, prompt and Settings come from its set. A tool it lacks is refused there before any side
  effect. The Mac's game is `controller/bridge_mac.rs`: only its own launch (launch id, Game session UUID, launcher birth, the log
  whose `Arg:` lines name its link) is ever read, looked at or closed; Home's VRChat is the person's. It starts through the start
  Home's Play shares (`obelisk_cli::Play`: one at a time, reserved by the bridge before it makes an instance, none within 80 s
  of an early end, by this run's exit, a deadline on the boot clock (`kern.bootsessionuuid`) and the engine's records; a record
  that cannot be read holds starts 80 s from first sight, then is history, except the current session's, which Home names with a
  **Set aside** button: never a lock for ever, never "nothing"; one slot for the computer, a lock beside the gate's file, held from
  before the gate is read again and the start decided until how its game ended is known and kept, also against this very N4N0, and
  while the wait after an early end cannot be written, until that wait is over; the lock's own file says a start from before it is
  made, so that an N4N0 that dies leaves it as an early end to whoever takes the slot next), only with obelisk-cli 0.1.14 or later (the record's `shown` frames
  are its renderer evidence, `--no-saved-arguments` what every start is made with, and before 0.1.14 VRChat could crash under heavy loading,
  `docs/mac-performance.md`; Home's Play holds to the same `obelisk::LEAST`), arrives only once the local player is in its own destination in its own held log, closes with
  `steam quit --session` and never `--force`. A stall, a quit that does not finish or a lost renderer of any session the engine
  runs is captured once (`controller/incident.rs`, `<logs>/incidents/`, Settings' Log says where): look there before guessing. VRChat's freeze about 5 s after the
  avatar loaded was Neptune's render server refusing an upload over 64 MiB: fixed in Orbiters-cc/neptune (branch `orbiters`) and
  shipped by obelisk-cli 0.1.4's engine patch (the fork's `docs/CLI.md`, Graphics); `renderer_lost`/`start_crashed` still report
  any other loss. Screenshots need macOS's Screen Recording for the tray service,
  which only the person can allow. A real game on this Mac: at most a few starts, never one within 80 s of an early quit, close it
  when done (`obelisk-cli steam quit 438100 --wait 60`), and know each start signs the owner's Steam in here and out on his PC.

- **A game starts on one complete released set of engine files or not at all, and they never change under it** (obelisk-cli
  0.1.11, its `docs/CLI.md`, Keyboard layout): the exact digests of every file (the manifest's `generations`) proven on every path
  that lets a launch go on, and every other file a start needs looked at (there, a file, its size, can be run). One lock for the
  whole library (`<library>/Voile.locks/library.lock`, outside what an install replaces): every launch, Rosetta's too, holds it
  shared as one open file from before its runtime is picked to its game's exit, inherited by its supervisor; every change (patch,
  recovery, install, repair, removal) holds it alone from before its look to its last check. No pin, no launch. The lock is the fast
  path, never the proof: nothing is replaced, put back or removed before the session records and the processes running a file of the
  library's runtimes (`voile/live.rs`) show that nothing of its engine runs; a look that failed or is not whole is never "nothing". A
  release is laid only once every member of it is what its archive holds (type, size, mode, link, SHA-256), before its place and in
  it. Only APFS: a repair is one atomic exchange or is refused, with no other way to make it, and nothing is laid anew there either. A refusal is the CLI's `engine_files` error, which Home shows
  with **Repair**: never tell anyone to delete a file of it. What a guest writes goes to its log through the supervisor
  (`game_log.rs`), by a blocking pipe that is always read: nothing of logging may ever block, refuse or end the game. voile-lkd `bf0af9b` (the attach stop as link state, the
  event ring) is **held back**: one start of five on it got Easy Anti-Cheat's "Debugger detected" (`docs/vrchat-bridge.md`,
  2026-11-23 08:06); never ship it before that is explained. **Accepted limits** (the architect's rulings after the seventh
  verification, 2026-11-23: not to be reopened without new evidence): a supervisor suspended by a debugger or `SIGSTOP` stalls the
  game's standard error once the pipe fills (as in 0.1.8; it takes someone to stop it, and a drainer of its own would change the
  process tree around the game): no new process for it, and no stopped-reader test; an unrelated process that maps a file under the
  library makes a repair refuse (the refusal names the process and the path where it can); the host-side helpers' output
  (`metal-HUD:`, `wsd`) is outside the guest's byte cap, bounded by their own counters; a start by 0.1.10 or older knows no library
  lock: what it races is covered by the look at what runs, and by the one release's locks it knows, and no further. Home offers
  **Restart VRChat** only for a confirmed lost picture of its own running session, the one it names (`controller/picture.rs`, R1;
  one decision per session, shared with the bridge and the incident capture; confirmed only by evidence dated after the drop, ended
  only by a counter that grew, and once confirmed never by the first count of a new swapchain): never make a covered, locked or slow game look lost,
  nor one that could not be read. Benchmarks: `tools/mac-bench/session.sh` (the screen must be unlocked; only valid
  windows are scored).
- **The engine patch (obelisk-cli's files over Voile's) must replace every earlier build's files.** Each manifest entry lists
  the SHA-256 every earlier `cli-v*` put at that path (`previous`); `scripts/build-engine-patch.sh` (from 0.1.8 reading every `cli-v*` release on GitHub) writes none that leaves one
  out, and `every_earlier_release_s_files_are_replaced` (with `OBELISK_PRISTINE_RELEASE`) checks it from the real tags. A file
  of ours loaded in the game's process (Neptune's `d3d11.dll`/`dxgi.dll`, the Wine driver) is changed only when necessary and
  soaked over several starts first. Measure VRChat on a Mac with `tools/mac-bench/bench.py` (`docs/mac-performance.md`): one
  results file per start, the screen unlocked and the window in front, or the shown-frame numbers mean nothing.

- **Every version bump gets a GitHub release.** `cargo xtask bump <version>`, commit and push, then
  `cargo xtask release` (with `--unsigned` until Azure Artifact Signing is set up; see `docs/releasing.md`). It tags
  `desktop-v<version>` and attaches `OrbitersSetup-<version>.exe` to
  https://github.com/Orbiters-cc/Orbiters-N4N0/releases (`cargo xtask github` redoes that step alone).
- **Every build Enzo is to try is on GitHub's releases AND in the development environment** (Enzo, 2026-10-25, no
  exception unless he clearly says otherwise in that request). `cargo xtask bump --dev`, commit, `cargo xtask release
  --dev`: it builds, puts the version in the development server's feed (127.0.0.1:4100, so his installed app with
  Settings, About, Development environment on shows Update beside its name within two minutes), pushes the branch and
  the contracts, tags `desktop-v<version>` and makes a GitHub pre-release with the setup at
  https://github.com/Orbiters-cc/Orbiters-N4N0/releases. Changing this tool and not posting a release there is a
  mistake. A folder of programs (`target\release`, `.orbiters-build`) is never "the build to try". The report links the
  GitHub release and names the version. Never ask him to confirm rights statements or publishing of his own assets:
  he owns all of it, and asking for a build/deploy is the approval.
- Project duplication must produce a usable independent copy, including installed dependencies and their exact
  versions, without asking whether dependencies should be included. Preserve working packages even when their
  repositories are unavailable; do not unnecessarily re-resolve or upgrade them. Audit local package references
  and links so the copy does not silently lose required files. Library is a regenerable cache, not a dependency.
- A retained project must be visible and actionable in the project gallery, including after restoration errors.
  Register completed copies before fallible restoration, refresh the gallery on failure, and report the exact
  destination and remaining problem. Never claim a copy is ready unless its required dependencies are present.
- Validate the complete user workflow, including failure/recovery and gallery visibility, rather than treating
  a successful filesystem operation as a completed feature. Add regression coverage for these outcomes.
- Duplication must measure required bytes including external dependencies, check destination free space, and keep
  Duplicate disabled until the check succeeds. Explain insufficient space and recheck immediately before copying.
- Progressive galleries must preserve the visible item and its scroll position as content arrives. Do not animate
  layout dimensions with springs or let background insertion move the user's viewport unexpectedly.
- **The SteamVR test sandbox is one shared, long-lived session** (`tools/vr-sandbox`, `docs/vr-sandbox.md`): take a
  turn with `python tools/vr-sandbox/sandbox.py acquire --owner <your stream> --app <your exe>`, use the VRChat that
  is already running (never close and relaunch it for an experiment: it costs minutes, and a launch within ~80 s of an
  early quit is what makes Easy Anti-Cheat fail with "EAC unload timeout"), and `release --owner <your stream>` as soon as you have your evidence. Do not `restore` or
  stop a session another agent holds. Enzo taking his headset back (`sandbox.py yield`, Virtual Desktop connecting)
  always wins; him using the mouse and keyboard is not a reason to stop.
- Unity storage repair must match log errors to the affected project, account for import workspace beyond copy
  size, and hold the Unity project lock before changing cache paths. Preserve Assets, Packages, ProjectSettings
  and exact dependency versions. Copy and verify cache contents before switching locations; retain the original
  on failures and test rollback. Do not close other Unity projects or delete unrelated caches to recover space.

## Several VRChat accounts on one computer

What is kept apart, and what is not (the architect's ruling, 2026-11-24, after the thirteenth verification). **How far this goes is
Enzo's decision, not taken yet: do not widen it without his word**; three rounds that did took working ways away from ordinary players.
- **A computer of one profile behaves as before VRChat's profiles existed** (0.3.9-dev.42), on Windows and on a Mac: a private place
  goes with the start, or straight into the game that runs; no game log is asked, and nothing is hidden, refused or withheld.
- **Several profiles** is plain evidence that can be taken back, and nothing else: a nonzero profile on Home's choice
  (`LaunchSettings::profiles`; its × takes it off), or one that runs or starts now, Home's or the bridge's
  (`vrchat_launch::several`, `Controller::launcher_several`). Never guess it from VRChat's folder, its logs or its saved values, nor
  from settings that could not be read, and never remember it for good.
- **Protected, on a computer of several profiles: the launch hand-off of a private place** (anything but a public instance, an invite,
  an instance made here for an account). It rides with no cold start: the profile chosen starts without it, that very VRChat's own log
  proves who is signed in (made at or after its process, at full precision, and no other's), and it is delivered on a match alone (a
  PC: the link into that very game, proven again right before; else the account's own invite to itself; a Mac: that invite). Otherwise
  nothing is sent, Home says why and keeps it for **Try again**. The one rule is `vrchat_launch::way`. The Mac's bridge, which cannot do
  the second step with nobody there, refuses its own starts (`several_profiles`).
- **Deliberately not kept apart today**: what this computer stores (the visit history and every list of it, captures and their
  places), whether a capture's instance is still open, the background presence and uploads to Orbiters, Orbiters' own account's invites,
  and the Windows bridge's main session. They act as the app's accounts did before profiles.

## Coordination notice, 2026-11-23: the VRChat bridge's files

Enzo decided to restructure where the VRChat bridge lives (`crates/orbiters-desktop/src/bridge/`, the
`controller/bridge_*.rs` adapters, `tools/vr-sandbox`). That work is being prepared away from `main` and will be merged
in one piece when no other session is landing bridge changes. Until Enzo says it is merged:
- Finished bridge work may still be pushed to `main`; say so to Enzo when a session has stopped pushing bridge
  changes, because the merge waits for that.
- Do not start large refactors or file moves in those paths on `main`; keep changes there small and self-contained.
- A release's version bump is now pushed BEFORE its long build, so two sessions no longer claim the same dev number
  (2026-11-22: four collisions). If your release flow pushes after the build, fetch and merge `origin/main` first.

## Owner decisions (Enzo's standing rulings: do not undo them, whoever asks, without his new explicit word)

- **The Quick Menu's side tiles stay while a page is open.** In VR and on the desktop, the Orbiters tiles beside VRChat's
  Quick Menu (the wing on either hand's side, or the strip under the menu in that layout, with their caption and Stop)
  stay visible and pressable while one of our pages (sheet) is up; the page stands clear of them. Enzo asked for this
  several times (2026-11-09, again 2026-11-13: "I asked multiple time to make the side tiles on the quick menu stay even
  when a page is open"); a 2026-11-11 change (F8, commit 2334ad22) removed them and is overruled. Only the person's own
  Hide puts them away; a page opened from the wheel shows alone (the ring's cells are not side tiles). Code:
  `Panels::set_open` (overlay/orbit/mod.rs), `vr::clear_of_sheet` and the places in `vr.rs`; tests
  `flows::the_side_tiles_stay_while_a_page_is_up`, `flows::the_side_tiles_stay_in_every_headset_layout`,
  `vr::tests::side_tiles_stay_beside_a_page`. Never delete or weaken these tests to make another change pass.
- **One row of page tabs, the same on every page** (2026-11-13). A page's head tabs come from `panels::tab_row` only:
  the six tools, icon and name, at the same places and size on every page in a language, the open one lit (a page
  reached from All tools lights All tools); while a capture shows, a recording runs or a status is up they are icons,
  again the same on every page. No page gets its own tab variant. Test: `panels::tests::one_tab_row_on_every_page`.
