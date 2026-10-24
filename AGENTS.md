# Working on Orbiters - N4N0

- **Every version bump gets a GitHub release.** `cargo xtask bump <version>`, commit and push, then
  `cargo xtask release` (with `--unsigned` until Azure Artifact Signing is set up; see `docs/releasing.md`). It tags
  `desktop-v<version>` and attaches `OrbitersSetup-<version>.exe` to
  https://github.com/Orbiters-cc/Orbiters-N4N0/releases (`cargo xtask github` redoes that step alone).
- **Say where the new program is.** After building the app, the report gives the path of the freshly built
  `target\release\orbiters-desktop.exe` (the one to launch) and, after a release, the setup's
  `target\dist\<version>\OrbitersSetup-<version>.exe`.
