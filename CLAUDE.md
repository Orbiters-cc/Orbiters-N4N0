Read AGENTS.md in the root of this repository before working here. Above all: every change to Orbiters - N4N0 that
Enzo is to try ends with `cargo xtask release --dev`, which posts a GitHub release at
https://github.com/Orbiters-cc/Orbiters-N4N0/releases and an update in the development environment.

- Project duplication must produce a usable independent copy, including installed dependencies and their exact
  versions, without asking whether dependencies should be included. Preserve working packages even when their
  repositories are unavailable; do not unnecessarily re-resolve or upgrade them. Audit local package references
  and links so the copy does not silently lose required files. Library is a regenerable cache, not a dependency.
- A retained project must be visible and actionable in the project gallery, including after restoration errors.
  Register completed copies before fallible restoration, refresh the gallery on failure, and report the exact
  destination and remaining problem. Never claim a copy is ready unless its required dependencies are present.
- Validate the complete user workflow, including failure/recovery and gallery visibility, rather than treating
  a successful filesystem operation as a completed feature. Add regression coverage for these outcomes.
- Follow AGENTS.md's disk-space preflight and gallery-stability requirements: disable duplication until its
  complete size is checked, and keep local scan updates independent from remote API requests and viewport position.
