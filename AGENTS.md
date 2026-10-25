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