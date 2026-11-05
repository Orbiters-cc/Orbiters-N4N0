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

Settings, Experimental, VRChat bridge. It then shows the address and the token, a command and a `.mcp.json` for Claude Code, whether a
client is connected, the recent actions, and a large Stop. For Claude Code:

```
claude mcp add --transport http orbiters-vrchat http://127.0.0.1:47615/mcp --header "Authorization: Bearer <token>"
```

Needed as well: signed in to VRChat in Settings (private instances are made with that session); the same account in the game; OSC on in
VRChat's Action Menu, Options, OSC; an avatar with N4N0's capture kit (else a recording has no keyframes).

## The tools

| Tool | What for |
| --- | --- |
| `get_state` | Everything readable, and `next`: what to do now. |
| `find_worlds` | World ids: search, visited, favorites, popular, active. |
| `go_to_world` | A new private instance of a world, and VRChat started in it (closing the last one). |
| `close_vrchat` | Closes the game the bridge started. Reconstructions then run. |
| `wait_for` | Waits for arrival, closing, a saved picture, reconstructions, the end of a pause. |
| `screenshot` | VRChat's window as a JPEG, with timestamp, sizes and scale. |
| `walk` | Over OSC, at most 10 s a call. Says the speed VRChat reported. |
| `turn` | Over OSC, about so many degrees, and says it is approximate. |
| `press` | One input for a short time: `jump`, `quick_menu`, axes over OSC; a short list of keys. |
| `mouse` | Moves the pointer or clicks in the window, or looks (relative). |
| `focus_game` | Asks Windows to bring the game forward: Windows may say no. |
| `take_3d_picture` | 1, 4 or 8 seconds, then the session and its reconstruction's state. |

The resource `bridge://guide` and the prompt `world_tour` explain the loop to an agent, including when to give up on a world. Every refusal has a
`code`, a `message` and a `next` (and `retry_after_s` when that is the answer).

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
  anyone else's. Anywhere else, movement, input, pictures and screenshots are refused and the agent is told; it can leave.
- **Keyboard and mouse only into the foreground game window.** Checked right before `SendInput`, and during a held key. Mouse positions must be inside the
  window. OSC needs no focus, and is the first choice. The bridge asks Windows to bring the game forward on arrival and through `focus_game`, only while the person is not paused-for (no
  input of theirs for 30 s): an unattended run cannot open the camera otherwise.
- **Screenshots are of that window only.** Windows Graphics Capture for that handle in a short-lived process (the service loads no GPU driver), `PrintWindow` for
  the same handle if that fails. Never the desktop.
- **Nothing types or speaks.** The inputs are an allow-list without `Y`, Enter, letters that type, `/input/Voice` or `/chatbox/*`. A click can reach anything on the game's
  menus, invites and avatars included: the guide and the tool's description forbid those, and nothing can enforce it.
- **The person always wins.** Any real key or mouse movement (Raw Input, whichever window has the focus, told from our own by having a device;
  `GetLastInputInfo` as a second opinion) pauses the agent and releases input; it goes on 30 s after the last of it. `Ctrl+Alt+F12` or Pause pause it until resumed. Stop
  turns the bridge off, ends what runs (a recording too), closes the port, and leaves the game as it is, no longer the bridge's. `bridge/person.rs`.
- **Nothing stays down.** A press lives in one call, at most 10 s, and comes up at its end, when the client disconnects, the person takes over, the instance stops
  being private or the focus goes. A sweep brings up anything past its time. If the app itself dies, `--bridge-guard`, told what is down on its standard input,
  brings it up when that input ends. `bridge/input.rs`, `bridge/guard.rs`.
- **Restraint with VRChat's API.** The session's own limiter (one request a second per family, 429 backoff across accounts), and at most one world every 30 s.
- **The GPU rules stand.** Reconstructions wait for VRChat as ever, and do not slip into the moments the game is closed between two worlds.

What it uses: startup arguments, OSC, ordinary window input, window capture, the game's log, and VRChat's API with the session the app already holds.

## How it starts VRChat

`steam.exe -applaunch 438100 --no-vr -screen-fullscreen 0 -screen-width 1280 -screen-height 720 [--profile=N] vrchat://launch?ref=orbiters.cc&id=<world>:<instance>`, else, without Steam, VRChat's own
`launch.exe` (from the `vrchat://` link it registers) with the same arguments. Never `VRChat.exe` itself: that goes around the launcher Easy Anti-Cheat starts from. `-applaunch`
because, unlike a `steam://` link, it asks nothing. No `--osc`: VRChat's defaults are what the app's OSC finds through OSCQuery. Changing worlds restarts the game, because
a running VRChat can only be moved by an invite an agent would have to accept in its menus.

## Testing

Nothing here may touch the person playing on the computer. `bridge/lab.rs` is a fake VRChat: a real window off every screen and never activated, found, captured and closed through the
real calls; a real UDP listener for OSC; a log in VRChat's format read back by the real `LogWatcher`; and a stand-in for `features/mine`, asked through the real `Api`. It records keyboard
and mouse input instead of sending it, and "having the foreground" is a switch for the same reason (off, the real check on the real window says no).

- `cargo test -p orbiters-desktop bridge::`: the rules and the whole loop over the real transport (`bridge/e2e.rs`).
- `node scripts/bridge-lab.mjs <orbiters-desktop.exe> <folder with @modelcontextprotocol/sdk>`: the same from MCP's official client against `--bridge-lab`, including killing the
  process mid-walk and hearing the guard release it.

## Trying it on the real game

Only a real run can tell these (nothing below has run against VRChat):

1. Steam passes the arguments, the game starts windowed at 1280 × 720 without VR, and the log follows it in. Enzo's Steam launch options (`--no-vr profile=3` in the logs) stay in force.
2. `vrchat://launch` as an argument through Steam takes it into the instance rather than Home.
3. `POST /instances` answers as the community specification says, with the session's account.
4. `/input/*` moves the avatar without the focus; how fast `LookLeft` and `LookRight` turn (90°/s is a guess).
5. `SendInput` reaches the game: scan codes for keys, `esc` opening the menu, the pointer landing where the pixels say, clicks registering.
6. Windows Graphics Capture and `PrintWindow` on the game's window (Easy Anti-Cheat running), and whether the yellow capture border shows.
7. Whether the game gets the foreground on arrival. `go_to_world` asks for it when nobody has touched the computer for 30 s, and says if Windows refused (`foreground: false`); then no menu can be opened and the camera stays closed until the person clicks the game.
8. Whether VRChat says `/usercamera/Pose` after a turn with the camera open, and whether the camera works in desktop mode for a 3D picture as it does in VR, Stream mode and Spout included.
9. Raw Input hearing the keyboard and mouse beside the game (and nothing else on this computer pausing it: anything that injects input counts as the person).
10. How long `WM_CLOSE` takes to close the game, and whether it asks anything.

With VRChat closed, signed in to VRChat in Settings, an avatar with the capture kit, turn the bridge on in Settings, run the command it gives you, restart Claude Code and paste:

> Use the orbiters-vrchat MCP server. Read bridge://guide. Then go to two worlds of your choice from my favorites, take one 4 second 3D picture in each, close VRChat, wait for the
> reconstructions, and tell me what happened at each step, including what you had to try twice.

What should happen: VRChat opens in a window at about 1280 × 720, in a private instance (the instance's name shows `~private(` in Settings' log and in `get_state`). The agent opens the menu with Esc,
clicks the camera, closes the menu, turns a little, walks, the camera flies, and N4N0 records. The game restarts into the second world. At the end it closes, and reconstructions start.

What to look at: did it stay out of your way until you touched something? Touch the mouse: it should pause at once, and go on 30 s later. Press `Ctrl+Alt+F12`: paused until pressed again. Press Stop: the
avatar stops at once and the port closes. Compare `turn 90` with the picture, and tell me the real rate. If a step fails, the agent's text and Settings' Recent actions say which: send those.
