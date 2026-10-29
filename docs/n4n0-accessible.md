# Make N4N0 accessible (plan, 2026-10-29)

Enzo's ticket: any VRChat player should get from "I want 3D captures" to captures shared on Orbiters with as little
friction as possible. This file splits it into workstreams, says which repository each touches and what is done.
Contracts that several programs share are in `contracts/Docs/project-setup.md` and `contracts/Docs/capture-sharing.md`:
read them before changing anything they describe, and change the contract first when it is wrong.

Repositories:
- Desktop: `H:\metaverse\ORBITERS\Orbiters-N4N0` (Rust, egui; this repo; `contracts` is a submodule).
- Server + website: `H:\metaverse\ORBITERS\server` (`backend` Node/Express/Sequelize, `frontend` React/HeroUI), branch
  `dev`, edited in place (no worktrees or copies of it), never pushed without Enzo asking.
- Unity packages: `H:\metaverse\unity projects\MCB Test\Packages\orbiters.toolkit`, `orbiters.myavatar` (each its own
  git repo; Unity editor shared by several agents: one compile error blocks all).
- World: `H:\metaverse\unity projects\Orbiters 3D pictures viewer` (VRChat world, UdonSharp; My World package
  `orbiters.myworld` lives there).

## Workstreams

| # | What | Where | State |
|---|------|-------|-------|
| A | Unity projects page (VCC/ALCOM replacement on `vrc-get-vpm`), project discovery, avatar cards, packages, repos, new/add project | desktop | |
| B | "Add to my avatar" (Capture page), setup requests | desktop + Toolkit + My Avatar | |
| C | Sharing API, moderation, explicit flag, avatar thumbnails, slots with eviction, presence | server backend | |
| D | Photos & captures page (rename of Sessions), Share publicly / Share to …, Visited worlds share morph, friends picker, faster upload preparation, explicit detection | desktop | |
| E | Website `/gallery` shares tab, 3D captures in the gallery modal, `/maps/wrld_…` shares, unshare | server frontend | |
| F | World: "I want to capture" menu, tutorials, capture avatar pedestal, N4N0 logo, shared captures row | world | |
| G | N4N0 on the avatar: share buttons after a capture, History page (screen version 12) | desktop menu + Toolkit | |
| H | VRChat sign-in in N4N0 behind the ADMIN-only feature `n4n0-vrchat-login`, token kept locally | desktop (+ server feature row) | |
| I | My Avatar: "Install N4N0 desktop" when the item is on an avatar but the app is missing | My Avatar | |
| J | My World: thumbnail creation (from My Avatar), Publish 3D captures (coverage gizmo, capture, share, "official previews"), per-platform display variants | My World + world + server | |
| K | Signed installer through SignPath (free for open source) | desktop xtask + CI; needs Enzo's SignPath account | |
| L | Moderation UI for ADMIN/MODERATOR (website + desktop) | server frontend | |
| M | Fixes: reconstruction ground level, recording without the avatar in view, menu touch/pointer freezes and re-centring loop, lazier follow on the hips, more detail on PC, 4-view grid shots while recording | desktop + Toolkit | |

## Order

C first (its API is what D, E, F, G use), A and D in parallel against the contract, B after A's VPM layer, E after C,
F and G after C, M in parallel with everything (independent), K whenever Enzo has a SignPath account.
