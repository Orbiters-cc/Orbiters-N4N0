# Working on Orbiters - N4N0

- **Windows and macOS (Apple silicon only) are both shipped.** Whatever VRChat gives on the same computer (Spout, SteamVR,
  its log) has nothing to read on a Mac (`orbiters_core::osc::VRCHAT_RUNS_HERE`), and a new feature that needs it is hidden
  there; one that goes over OSC talks with the VRChat on the network (`orbiters_core::lan`). Native code goes behind
  `cfg(target_os = "macos")` beside the `cfg(windows)` path. `cargo xtask` on a Mac makes the Mac's release:
  `target/dist/<version>/Orbiters-N4N0-<version>-macos-arm64.dmg` is the build to give (see `docs/releasing.md`).

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
- Unity storage repair must match log errors to the affected project, account for import workspace beyond copy
  size, and hold the Unity project lock before changing cache paths. Preserve Assets, Packages, ProjectSettings
  and exact dependency versions. Copy and verify cache contents before switching locations; retain the original
  on failures and test rollback. Do not close other Unity projects or delete unrelated caches to recover space.

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
