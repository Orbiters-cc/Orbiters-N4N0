# Unity projects implementation validation — 29 October 2026

Local implementation on `main`. No version bump, tag, push, release or modification of the contracts submodule.
The installed application has not been updated; the coordinator owns the development release.

## Commits and files

- `a362e9d` — Add shared Unity project discovery and VPM services.
- `375cb23` — Add the Unity projects page and Capture avatar setup flow.
- The validation commit adds this report, the usage document, screenshot scenarios and progressive-cache persistence.

New crate: `crates/orbiters-projects`. Its modules are `discovery`, `avatars`, `vpm`, `templates`, `builtin_templates`, `network`, `setup`
and the shared helpers in `lib.rs`; `templates/` contains the pinned MIT-licensed fallback assets. It also has the read-only `examples/discover.rs`, contract tests and a real VPM
lifecycle test backed by a loopback HTTP repository. New desktop files are `pages/projects/{mod,state,ui}.rs`.
Integration changes are limited to navigation/page dispatch, Capture's avatar setup entry, the shared activation
helper and the server-specific N4N0 gallery ID constant. Documentation and third-party notices/licenses are updated.
The other agent's Photos & captures/Visited worlds files and changes are excluded from these commits.

## Checks

Every Cargo invocation used:

```powershell
$env:CARGO_TARGET_DIR='C:/Users/Enzo/.orbiters-target'
$env:CARGO_INCREMENTAL='0'
```

- `cargo test -p orbiters-projects` — **12 tests passed**, no failures (11 contract/discovery/template tests, 1 lifecycle test).
  The lifecycle test performs actual ZIP downloads and dependency installation, upgrade, downgrade, restore,
  conflicting-removal refusal, repository headers/visibility/removal, creation and duplication. All writes go to
  disposable stores and projects; no real projects or VCC registration were changed.
- `cargo check -p orbiters-desktop` — passed. The remaining dead-code warning concerns existing upload methods in
  `backend.rs`, outside this workstream.
- `cargo build -p orbiters-desktop` — passed; its offscreen renderer produced all required states. One retry was
  needed when the shared output executable was temporarily locked by another renderer. No running installed app
  was stopped or replaced.
- `cargo fmt -p orbiters-projects` and targeted rustfmt with this repository's config — applied. Desktop formatting
  was restricted to owned files to preserve the concurrent agent's edits.
- `python scripts/third_party_licenses.py` — regenerated. The concurrent image/WebP dependency's license/lockfile
  hunks remain with that workstream; only this workstream's dependency changes were staged.
- `git diff --check` and staged diff checks — passed.
- Development `GET /vpm/known` and `GET /vpm/public` — both returned successfully with empty lists.

## Visual review

The final PNGs under `C:/Users/Enzo/.orbiters-project-review/final/` were opened and visually inspected:

- `projects-gallery.png` (1280 × 1000, including the proposals' Add buttons).
- `projects-detail.png`, `projects-packages.png`, `projects-repos.png`, `projects-new.png`.
- `projects-avatars.png`, `projects-choice.png`, `projects-setup.png`, `projects-ready.png`.
- `home.png`, as the adjacent style reference.

The two 1024 × 760 images under `C:/Users/Enzo/.orbiters-project-review/narrow/` (`projects-new.png` and
`projects-avatars.png`) were inspected too. Dialog heights and package-row density were refined after the first
review. These are deterministic stand-ins, with illustrative avatar placeholders, not a live project installation.

Reproduce a state with:

```powershell
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-avatars
```

## Machine discovery

The read-only example uses the real VCC store, Unity registry and fixed local volumes. Its detailed output is kept
in `C:/Users/Enzo/.orbiters-project-review/discovery.json` and `discovery-progress.log`. Before/after SHA-256 records
for the real VCC `settings.json` and `vcc.liteDb` are `vcc-before.json` and `vcc-after.json`; they match.

The nearby scan found **2 registered projects and 69 proposals** before full-disk enumeration. The complete run
found **2 registered projects, 153 proposals and 313 avatar occurrences**. The full disk walk took **680.560 seconds**
(11 min 20.56 s), over **506,117 directories** across C:, D:, F:, G:, H: and I:, with **no warnings**. The complete run,
including avatar parsing, took **817.857 seconds** (13 min 37.86 s). The scan's only persistent writes are Orbiters' own
discovery cache; it never installs into or modifies real projects.

## Contract and acceptance limits

- Setup request field names, gallery item shape, atomic filename and result statuses match the binding contract.
  Results remain on disk until displayed, then are consumed on a background worker.
- Production's gallery ID is deliberately unset as requested; development uses 30. The contract has no environment
  field even though gallery IDs are server-scoped, so desktop and My Avatar must use the same environment.
- Scene extraction supports direct PipelineManager components and the Canine scene's prefab override structure.
  Arbitrarily deep nested-prefab inheritance is not fully resolved and can omit avatars whose ID is inherited only
  through multiple prefab levels. Binary scenes are excluded.
- Installed VCC templates and VCC-format user templates are supported. A fresh install without VCC uses bundled
  ALCOM Avatar/World/Base assets (MIT, pinned v1.9.3); the fresh-install materialization is tested. ALCOM-only custom
  template hooks are not interpreted.
- Shared-store mutations and full package operations passed in disposable fixtures. Running VCC/ALCOM together,
  authenticating thumbnail fallback against a populated server, public repository installation, Toolkit's actual
  confirmation and the final VRChat SDK upload have **not** been exercised. No claim of release or end-to-end Unity
  validation is made.

To see the implementation, use **Unity projects** immediately after Capture, or **Capture → Add to my avatar**.
The coordinator must publish the development update before these appear in the installed application.
