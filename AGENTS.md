# Working on Orbiters - N4N0

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
