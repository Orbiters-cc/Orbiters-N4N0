# Releasing Orbiters desktop

A release is a version bump, built on this computer (not on GitHub's runners) and published on orbiters.cc:

```sh
cargo xtask bump 0.2.0          # the workspace's version, in Cargo.toml and Cargo.lock
git commit -am "Orbiters desktop 0.2.0" && git push
cargo xtask release
```

`release` refuses a commit that is not clean, not pushed, not on `main`, or whose version is already released. It
then builds and signs everything (`dist`), uploads it (`publish`), and tags the commit `desktop-v0.2.0`.

Installed apps look for a new version 45 seconds after they start and every 4 hours after that, or when the person
clicks **Look now** in Settings, About. They download the setup in the background and check its size and SHA-256
against the signed release. Then Settings and the sidebar offer **Restart to update**. If a recording, a
reconstruction, a call or an upload is running, the restart waits for it to end, unless the person chooses
**Restart now**. A downloaded update that was never installed is installed the next time the app starts. The update
replaces the files in place, then starts the app again: no old versions are kept beside it.

## What each command does

| Command | What it does |
|---|---|
| `cargo xtask keygen` | Creates the release key, once. The private half goes to `~/.orbiters-release/update-signing-key`, the public half to `crates/orbiters-update/update-key.pub`, which is committed and built into every app. |
| `cargo xtask bump <version>` | Sets `[workspace.package] version` and updates Cargo.lock. Every program and the release carry this version. |
| `cargo xtask dist` | Writes `target/dist/<version>/` (details below). `--unsigned` makes a test build, and `--programs <folder>` packs programs already built there instead of building them. Neither gets a release file, so `publish` cannot put them out. |
| `cargo xtask publish` | Uploads `target/dist/<version>/` to the server and makes it the channel's release (`--force` publishes a version that is not newer than the current one). Use it again after an upload that was cut. |
| `cargo xtask release` | Runs the checks, `dist`, `publish`, then the tag. `--notes <file>` adds release notes (plain text, shown in the app's Settings). |

All of them take `--channel <name>`, `stable` by default. An app follows the channel its install record names.

### `dist` in detail

1. Builds the five programs in release (`target/release`, like any other release build).
2. Builds the setup without programs inside, with the C runtime linked in (`+crt-static`, in its own target folder).
   This is the uninstaller kept in the install folder, so it must start on any Windows.
3. Gathers the programs, `onnxruntime.dll`, `onnxruntime_providers_shared.dll`, `sherpa-onnx-c-api.dll`, Visual
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
runs it, in `%LOCALAPPDATA%\Programs\Orbiters desktop` (or a folder they choose), without administrator rights. It
adds the app to Windows' installed apps (Settings, Apps) with its icon, version and size, a Start menu shortcut,
optionally a desktop shortcut, and optionally a start with Windows in the tray (`--hidden`).

- **Over an existing install:** it updates the install where it is, keeping its options.
- **While the app runs:** it asks the app to quit, as the app's own Quit does. The app finishes saving a recording
  first. After 20 seconds the setup offers to close it, warning that unsaved work is lost.
- **How files are replaced:** the payload is unpacked and checked inside the install folder first, then each file is
  moved into place. If a move fails, the files already moved are put back and the error is shown.
- **Uninstalling:** Windows runs `orbiters-setup.exe --uninstall` from the install folder. It copies itself to the
  temporary folder, then removes the app's Windows entries and only the files its install record lists. Settings,
  caches and speech models go too if the person asks. Their sessions are never touched.

| Arguments | What they do |
|---|---|
| (none) | The install window |
| `--silent [--dir <folder>]` | Installs with the default choices, no window |
| `--update --dir <folder> [--relaunch] [--hidden]` | What the app runs to update itself |
| `--uninstall [--silent]` | Uninstalls |
| `--screenshots <folder>` | Renders every page offscreen, for review |

If the setup's window cannot open (no usable graphics), the same steps run with Windows' message boxes instead.
