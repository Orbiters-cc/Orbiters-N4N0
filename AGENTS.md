# Working on Orbiters - N4N0

- **Every version bump gets a GitHub release.** `cargo xtask bump <version>`, commit and push, then
  `cargo xtask release` (with `--unsigned` until Azure Artifact Signing is set up; see `docs/releasing.md`). It tags
  `desktop-v<version>` and attaches `OrbitersSetup-<version>.exe` to
  https://github.com/Orbiters-cc/Orbiters-N4N0/releases (`cargo xtask github` redoes that step alone).
- **Hand over builds through the development environment** (Enzo, 2026-10-25). What Enzo is to try goes out as a
  development build: `cargo xtask bump --dev`, commit, `cargo xtask release --dev` (no push, no tag; the development
  server on 127.0.0.1:4100 serves it). His installed app, switched to the development environment (Settings, About),
  shows Update beside its name within two minutes. A folder of programs (`target\release`, `.orbiters-build`) is never
  "the build to try": he would end up trying an older version without knowing. The report says the version published,
  and a release for everyone (`cargo xtask release`) the setup's `target\dist\<version>\OrbitersSetup-<version>.exe`.
