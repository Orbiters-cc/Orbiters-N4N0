# Releasing Orbiters - N4N0

A release is a version bump, built on this computer (not on GitHub's runners), published on orbiters.cc and as a
release of [Orbiters-cc/Orbiters-N4N0](https://github.com/Orbiters-cc/Orbiters-N4N0/releases) with the setup attached:

```sh
cargo xtask bump 0.2.0          # the workspace's version, in Cargo.toml and Cargo.lock
git commit -am "Orbiters - N4N0 0.2.0" && git push
cargo xtask release             # --unsigned until code signing is set up (GitHub only)
```

`release` refuses a commit that is not clean, not pushed, not on `main`, or whose version is already released. It
then builds and signs everything (`dist`), uploads it (`publish`), tags the commit `desktop-v0.2.0`, and makes that
tag's GitHub release with `OrbitersSetup-0.2.0.exe` (`github`, through the signed-in `gh`). Every version bump gets
its GitHub release. With `--unsigned`, before Azure Artifact Signing is set up (below), the setup carries no
signature and goes on GitHub only: SmartScreen warns about it, and installed apps update only to signed releases.

Installed apps look for a new version 45 seconds after they start and every 4 hours after that, or when the person
clicks **Look now** in Settings, About. They download the setup in the background and check its size and SHA-256
against the signed release. Then Settings and the sidebar offer **Restart to update**. If a recording, a
reconstruction, a call or an upload is running, the restart waits for it to end, unless the person chooses
**Restart now**. A downloaded update that was never installed is installed the next time the app starts. The update
replaces the files in place, then starts the app again: no old versions are kept beside it.

## On a Mac

The same commands, run on a Mac, make and publish the Mac's release:

```sh
cargo xtask bump --dev              # or a version
git commit -am "Orbiters - N4N0 0.3.4-dev.32" && git push
cargo xtask release --dev
```

`dist` writes in `target/dist/<version>/`: `Orbiters - N4N0.app`, `Orbiters-N4N0-<version>-macos-arm64.dmg` (the
application and a link to Applications, what people download), `Orbiters-N4N0-<version>-macos-arm64.zip` (what installed
apps update with) and, when this computer has the release key, the signed `<channel>.json` that names the zip. `github`
attaches the disk image and the zip to the version's release, beside Windows' setup if it is already there (it does not
replace it), and Windows' `github` does the same the other way around.

Mac releases are in the feed's `macos/` folder (`<feed>/macos/<channel>.json`, the files under
`macos/<version>/`): an app only ever hears of its own platform's. They need nothing more from the server: Caddy already
serves `docker-data/downloads` as files. `xtask publish` uploads them with SSH as Windows' are.

**Signing.** `dist` signs with the best identity the keychain has, or `macos_identity` in the settings (`-`: ad hoc):

| Identity | What people see |
|---|---|
| Developer ID Application, notarized (`macos_notary_profile`, from `xcrun notarytool store-credentials`) | The app opens normally, anywhere. **Needs Apple's Developer Program; the only one to give the public.** |
| Apple Development | Gatekeeper blocks it where it was downloaded by hand, until allowed in System Settings › Privacy & Security › Open Anyway. Permissions and the keychain survive updates, since the signature's identity stays. |
| Ad hoc | The same, and macOS asks for the microphone, the local network and the keychain again after each update. |

The keychain asks for the signing key at each file unless told **Always Allow** once. `dist` first signs a small file to see
whether it gets an answer within a minute; without one, nobody being at the screen, it signs ad hoc and says so, rather than
wait (unless `macos_identity` names an identity: then it stops with an error). `cargo xtask dist` and `cargo xtask github`
again later replace the release's files with properly signed ones.

**The release key** is the same as Windows' (installed apps trust `update-key.pub`), so a Mac makes a signed release only
once it holds a copy (`~/.orbiters-release/update-signing-key`). Without it, `dist` says so and makes no release file:
`release --dev` then puts the files on GitHub only, and installed Macs do not hear of it.

**The update itself.** The app downloads the zip in the background and checks its size and SHA-256 against the signed
release. Restart to update starts the program again (`--apply-update`, `mac_update.rs`), from inside the application it
replaces, since macOS kills a copy of a hardened program elsewhere: it waits for the app to quit, unpacks the zip beside it, checks its bundle identifier and code signature, swaps the
applications with two renames (the old one goes back if the second fails) and opens the app again.

## What each command does

| Command | What it does |
|---|---|
| `cargo xtask keygen` | Creates the release key, once. The private half goes to `~/.orbiters-release/update-signing-key`, the public half to `crates/orbiters-update/update-key.pub`, which is committed and built into every app. |
| `cargo xtask bump <version>` | Sets `[workspace.package] version` and updates Cargo.lock. Every program and the release carry this version. |
| `cargo xtask dist` | Writes `target/dist/<version>/` (details below). `--unsigned` makes a test build, and `--programs <folder>` packs programs already built there instead of building them. Neither gets a release file, so `publish` cannot put them out. |
| `cargo xtask publish` | Uploads `target/dist/<version>/` to the server and makes it the channel's release (`--force` publishes a version that is not newer than the current one). Use it again after an upload that was cut. |
| `cargo xtask release` | Runs the checks, `dist`, `publish`, the tag, then `github`. `--notes <file>` adds release notes (plain text, shown in the app's Settings and on GitHub). `--unsigned`: no Authenticode signatures, no `publish`. |
| `cargo xtask github` | Makes the GitHub release of `desktop-v<version>` (the tag pushed already) with `target/dist/<version>/`'s setup, or replaces the setup of one that exists. |

All of them take `--channel <name>`, `stable` by default. An app follows the channel its install record names.

### `dist` in detail

1. Builds the five programs in release (`target/release`, like any other release build).
2. Builds the setup without programs inside, with the C runtime linked in (`+crt-static`, in its own target folder).
   This is the uninstaller kept in the install folder, so it must start on any Windows.
3. Gathers the programs, the face detector (`crates/orbiters-reconstruct/assets/faces-v1-int8.onnx`), `onnxruntime.dll`, `onnxruntime_providers_shared.dll`, `sherpa-onnx-c-api.dll`, Visual
   C++'s `vcruntime140.dll`, the licences and the uninstaller. Every program needs `vcruntime140.dll`, which a clean
   Windows lacks, so it ships beside them (app-local, as Microsoft allows). Each `.exe` and `.dll` is then checked:
   it may load only DLLs that come with Windows or ship beside it. If anything else is needed, `dist` stops.
4. Signs our programs (Azure Artifact Signing), then packs everything into a zstd-compressed tar whose index lists
   each file's size and SHA-256.
5. Builds the setup again with that payload inside, checks it needs only Windows' DLLs, and signs it:
   `OrbitersSetup-<version>.exe`.
6. Writes `<channel>.json`, the release (version, date, commit, notes, the setup's size and SHA-256), signed with the
   release key. `dist` checks that the key matches the one in `update-key.pub` first, because apps would refuse a
   release signed with any other key.

## First-time set-up

### The release key

```sh
cargo xtask keygen
```

Commit `crates/orbiters-update/update-key.pub`. Keep a copy of `~/.orbiters-release/update-signing-key` offline,
for example in a password manager:

- If it is lost, installed apps never accept another update. The only way out is a new key and every user
  reinstalling by hand.
- If it leaks, anyone who can also serve files from orbiters.cc can push code to every installed app.

The server itself holds no key. Without the private key, a compromised server cannot make an app install anything.

### Code signing (Azure Artifact Signing, formerly Trusted Signing)

Without an Authenticode signature, Windows' SmartScreen warns about the setup ("Windows protected your PC").

1. In the Azure portal, create an **Artifact Signing** account, complete the identity validation, and create a
   **Public Trust** certificate profile.
2. Give your Azure user the account's **Certificate Profile Signer** role (listed as Artifact Signing or Trusted
   Signing Certificate Profile Signer, depending on the portal's naming).
3. Install the Windows SDK (for `signtool.exe`), the Azure CLI, and the signtool plug-in: the
   `Microsoft.ArtifactSigning.Client` NuGet package. Its `bin/x64/Azure.CodeSigning.Dlib.dll` is what `dist` needs.
4. Write the account file, for example `~/.orbiters-release/signing.json`:
   ```json
   { "Endpoint": "https://<region>.codesigning.azure.net", "CodeSigningAccountName": "<account>", "CertificateProfileName": "<profile>" }
   ```
5. Sign in with `az login` before each release. The plug-in uses the Azure CLI's sign-in.

### Settings

`~/.orbiters-release/config.json`. Every field is optional, and each environment variable overrides its field:

```json
{
  "signing_dlib": "C:/tools/artifact-signing/bin/x64/Azure.CodeSigning.Dlib.dll",
  "signing_metadata": "C:/Users/<you>/.orbiters-release/signing.json",
  "setup_target_dir": "C:/Users/<you>/.orbiters-target"
}
```

| Field | Variable | Default |
|---|---|---|
| `update_key` | `ORBITERS_UPDATE_KEY` | `~/.orbiters-release/update-signing-key` |
| `signing_dlib` | `ORBITERS_SIGNING_DLIB` | none (no signing) |
| `signing_metadata` | `ORBITERS_SIGNING_METADATA` | none (no signing) |
| `signtool` | `ORBITERS_SIGNTOOL` | the newest x64 signtool of the Windows SDK |
| `timestamp_url` | `ORBITERS_TIMESTAMP_URL` | `http://timestamp.acs.microsoft.com` |
| `vcruntime` | `ORBITERS_VCRUNTIME` | Visual Studio's redistributable `vcruntime140.dll`, else System32's |
| `host` | `ORBITERS_RELEASE_HOST` | `blackorbit@orbiters` (the server over Tailscale SSH) |
| `remote_dir` | `ORBITERS_RELEASE_DIR` | `/home/blackorbit/Orbiters/server/docker-data/downloads/desktop` |
| `setup_target_dir` | `ORBITERS_SETUP_TARGET_DIR` | the target folder (the static setup needs about 1.1 GB of its own) |
| `macos_identity` | `ORBITERS_MACOS_IDENTITY` | the best identity the keychain has (`-`: ad hoc) |
| `macos_notary_profile` | `ORBITERS_MACOS_NOTARY_PROFILE` | none: Developer ID builds are not notarized |

### The server

The server repository serves `docker-data/downloads` at `https://orbiters.cc/downloads/` (`Caddyfile.prod`, mounted
in `docker-compose.yml`). `scripts/deploy/prepare-runtime-state.sh` creates `docker-data/downloads/desktop` as the
deploy user. Create it yourself if the deploy has not run since, because a folder Docker creates for a missing mount
belongs to root and the upload could not write there. Recreate the Caddy container once so it picks up the new
mount: `docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --no-deps caddy cloudflared`.

| Address | What it is | Cache |
|---|---|---|
| `/downloads/desktop/stable.json` | The signed release that installed apps read | `no-cache` |
| `/downloads/desktop/<version>/OrbitersSetup-<version>.exe` | Each version's setup | a year, immutable |
| `/downloads/desktop/OrbitersSetup.exe` | The latest stable setup | 5 minutes |
| `/download/desktop` | The link to give people: redirects to the latest setup | |

`publish` uploads each file under a temporary name and renames it once it is complete. The release file goes last,
so no app hears of a version before its setup is in place. Uploads go over SSH, not through Cloudflare, which caps
request bodies at 100 MB.

## The setup

`OrbitersSetup-<version>.exe` holds the whole app (about 30 MB) and works offline. It installs for the person who
runs it, in `%LOCALAPPDATA%\Programs\Orbiters - N4N0` (or a folder they choose), without administrator rights. It
adds the app to Windows' installed apps (Settings, Apps) with its icon, version and size, a Start menu shortcut,
optionally a desktop shortcut, a start with Windows in the tray (`--hidden`), and a start with SteamVR: the app's
SteamVR manifest (`orbiters-n4n0.vrmanifest` in its settings folder, key `cc.orbiters.n4n0`) registered among SteamVR's
startup overlay apps while SteamVR runs; when it does not, the app's settings keep the choice and the app tells
SteamVR the next time it runs. Both can be changed later in the app's Settings, Start.

- **Over an existing install:** it updates the install where it is, keeping its options.
- **While the app runs:** it asks the app to quit, as the app's own Quit does. The app finishes saving a recording
  first. After 20 seconds the setup offers to close it, warning that unsaved work is lost.
- **How files are replaced:** the payload is unpacked and checked inside the install folder first, then each file is
  moved into place. If a move fails, the files already moved are put back and the error is shown.
- **Uninstalling:** Windows runs `orbiters-setup.exe --uninstall` from the install folder. It copies itself to the
  temporary folder, then removes the app's Windows entries, its SteamVR manifest (and SteamVR's start of it, when
  SteamVR runs) and only the files its install record lists. Settings,
  caches and speech models go too if the person asks. Their sessions are never touched.

| Arguments | What they do |
|---|---|
| (none) | The install window |
| `--silent [--dir <folder>]` | Installs with the default choices, no window |
| `--update --dir <folder> [--relaunch] [--hidden]` | What the app runs to update itself |
| `--uninstall [--silent]` | Uninstalls |
| `--screenshots <folder>` | Renders every page offscreen, for review |

If the setup's window cannot open (no usable graphics), the same steps run with Windows' message boxes instead.
