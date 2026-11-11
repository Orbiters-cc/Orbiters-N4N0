# Camera web engine (Windows only)

Add `orbiters-web = { path = "../orbiters-web" }` under the desktop crate's
`[target.'cfg(windows)'.dependencies]`. No macOS fallback is exported.

`WebPage::new(960,720)` starts a COM STA worker. `navigate("https://...")` and
`resize(width,height)` queue commands, returning an error on a full mailbox or
invalid arguments. Sizes are bounded to 64..1920 per dimension. There is no
desktop window, activation, OS pointer movement or keyboard focus request.

On each overlay turn, call `poll()`. It uses try_lock: None means retain the last
snapshot. Some returns `(State, Option<Frame>, changed)`. Upload a frame only
when changed is true. Pixels are tightly packed, top-down, straight RGBA8;
`rgba` is an Arc slice, so retaining the frame does not copy its pixels.
Navigation clears the old frame. Frames are gated until the current navigation completes;
the chrome must still show the loading state. Display the `title` and `url` as
untrusted text, never as markup. Load is Loading, Loaded, Failed(reason), Closed.
Reasons are developer diagnostics; camera UI supplies localized friendly errors.
Always retain the explicit Open in browser action, including on runtime failure.
This crate never opens the desktop browser.

`input(Input::Move/Press/Release {x,y})` and `Wheel {x,y,dx,dy}` use content pixels,
independent of the chrome. Positive wheel deltas scroll right/down. Input is
delivered with Chromium's Input protocol, including press/release and dragging,
without activating an HWND. Map laser UVs to current content dimensions. Retry
commands rejected by the bounded mailbox on the next turn, especially releases.
Only send stick scrolling while this view is the intended input target.

`close()` and Drop set an atomic flag and never join. Poll until Closed if a
caller needs confirmation that the temporary profile was deleted. The application
can await Closed during normal shutdown. An independent hidden PowerShell watcher also
cleans profiles and exact-profile browser processes after host termination. Startup reclaims
stale sessions strictly under Temp/Orbiters-QR-Web-owned-v1. It never sweeps other Temp folders.
Slow navigation and Chromium page processes cannot block an overlay turn.

## Rendering and isolation

Pinned webview2-com 0.39.1 statically links Microsoft's loader; no loader DLL or
browser runtime is bundled. Evergreen must already be installed (present on this
Windows 11 host). Failure is reported asynchronously through Failed.

A composition controller uses a message-only HWND, with no desktop visual tree.
Chromium Page.screencastFrame supplies PNGs on paint, decoded on the worker;
acks are limited to 10 per second, identical pixels do not advance sequence.
Software rendering (`--disable-gpu`) avoids loading the VR GPU. This is intended
for browsing QR pages, not video or WebGL throughput. There is no WGC desktop
capture, screen permission prompt, capture border or visible hidden-window trick.

Each instance gets an InPrivate controller in a unique throwaway profile, never the Orbiters/VRChat/browser
profile. OS SSO, autofill, password saving, host objects, web messages, devtools,
default context menus and script dialogs are disabled. Permissions, downloads,
new windows, external protocols, basic authentication, client certificates and
invalid server certificates are refused. File chooser dialogs are intercepted.
Document-start guards disable printing and File System Access picker functions.
Top-level and frame navigations accept only http/https URLs without credentials.
No remote debugging port is opened. Browser renderer processes retain the
runtime's normal sandbox; the app itself must not be assumed sandboxed.

## Proof

Run through the shared deskcargo queue:

```
cargo run -p orbiters-web --example proof -- C:/Users/Enzo/.orbiters-build/vr-feedback-1111/shots-camera-web
cargo test -p orbiters-web -p orbiters-i18n
```

The proof starts an ephemeral loopback HTTP fixture, validates the loaded pixels,
scrolls to a known section and saves a different PNG, clicks a fixed link and
checks URL/title/pixels, resizes, reports a refused connection and closes the
controller/profile and fixture server. It never starts VRChat, SteamVR or Orbiters.
Inspect all three PNGs. See done-camera-web.md in the feedback folder for measured
results and limitations. The camera-ui stream owns placement, chrome, localized
wording, desktop-browser action and VR validation.

To test unavailable-runtime reporting without uninstalling anything, set
`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` to a nonexistent directory **only in the
proof process environment**, then run `proof OUTPUT --expect-unavailable`.
Close remains nonblocking after this startup failure; no browser was created.
The failed diagnostic remains available (Closed is the normal cleanup outcome).

## Review hardening

Public API types and signatures are unchanged. The changed bit means frame changes only;
inspect State on every successful poll. Titles are limited to 512 UTF-8 bytes, URLs to 8192,
and diagnostics to 1024, with control and bidi formatting removed before publication.
All async startup/CDP operations have cancellation checks and 15-second deadlines.
Required guard/capture/input completion failures close the engine rather than being ignored.
The effective UserDataFolder must equal the canonical owned folder before controller creation.
Browser command-line network overrides and remote debugging flags are rejected before navigation.

Pages use a mandatory local HTTP proxy with loopback bypass disabled. Each upstream connection
resolves the destination, rejects the entire answer set if any address is nonpublic, and connects
to the checked IP. This covers redirects, workers, HTTPS and secure WebSockets without a DNS
rebinding gap. QUIC, WebTransport and nonproxied WebRTC UDP are disabled. LAN/loopback QR pages
are intentionally unavailable here; Open in browser remains a separate trusted UI action.
Plaintext WebSocket upgrades are refused. No LAN opt-in or legacy network policy is provided.

The earlier `proof` example predates the public-network policy: its loopback rendering fixture is
now expected to be denied. Use `cargo run -p orbiters-web --example review` for current lifecycle
and blocked-network runtime checks, plus `cargo test -p orbiters-web -p orbiters-i18n`.
