# Sound devices in VR

Settings › **When SteamVR starts** chooses the sound output and input Windows uses by default while SteamVR runs
(Enzo's case: the SteamVR/headset device rather than Virtual Desktop Audio, which Virtual Desktop makes the default and
which sounds much worse). Each is **Don't change** or one of Windows' devices that are there now; a chosen device that
is not there shows as "(not connected)" and stays chosen. **Test** plays the interface's Reveal sound on the chosen
output only (whatever the default is); the input gets the same microphone test as Performance › Your voice. **Put back
the devices before** (on by default) restores the earlier defaults when SteamVR closes. The section's subtitle says
what happened last ("Set to Speakers (Valve Index) and Microphone (Valve Index) at 21:03", "… was not there, Windows
kept …", "Windows did not take …").

## How it works

- `crates/orbiters-core/src/settings.rs`: `Settings::vr_audio` (`VrAudioSettings`: `output`, `input` as
  `Option<AudioEndpoint { id, name }>`, `restore`).
- `crates/orbiters-desktop/src/audio_devices.rs`: Windows. Devices and defaults come from `IMMDeviceEnumerator` (active
  endpoints, `PKEY_Device_FriendlyName`). Setting a default uses `IPolicyConfig::SetDefaultEndpoint` from
  `CPolicyConfigClient` (CLSID `870af99c-171d-4f9e-af0d-e63df40c2bc9`, IID `f8679f50-850a-41cf-9c72-430f290290c8`):
  undocumented, but it is what Windows' Sound settings use, unchanged from Windows 7 to 11, and what SoundSwitch,
  EarTrumpet and nircmd call. Its vtable is declared slot by slot (IUnknown's three, ten slots kept as placeholders,
  then `SetDefaultEndpoint(PCWSTR, ERole)`, then `SetEndpointVisibility`); only `SetDefaultEndpoint` is called. The
  property store and `IMMDevice::OpenPropertyStore` are declared the same way so the `windows` crate needs no new
  features. Every role is set: console, multimedia and communications.
- `crates/orbiters-desktop/src/controller/vr_audio.rs`: the logic (`Keeper`, over a `Devices` trait mocked in its
  tests) and the tray service's `vr-audio` thread (started in `service.rs`). It reads the service's snapshot
  (`steamvr.running`/`pid`, the process watcher's `vrserver`) when SteamVR starts or stops or the choice changes, every
  second while it watches closely, and every 15 s outside VR while a device is chosen.
  - **SteamVR starts** (or its process id changes, or the choice changes while it runs): for 40 s the defaults are
    checked every second and the chosen device is set wherever it is not every role's default. This covers Virtual
    Desktop and SteamVR setting their own a few seconds after the start, and devices that only appear once SteamVR has
    the headset. Each device is set at most 5 times per window, so a program that keeps setting its own is not fought
    forever; after the window, a change the person makes stays. A device still missing at the end is reported.
  - **The devices before** are saved the first time a device is set: the defaults that held for 45 s outside VR (so
    not Virtual Desktop Audio put there just before Virtual Desktop launched SteamVR), otherwise the defaults at that
    moment. With a look only every 15 s, a default's time is counted from the last look that saw another one, and a
    default replaced counts as held until the look that sees the new one; SteamVR's start brings one more look. So a
    device the person used for 45 s before VR is never missed, whatever the pace, and Virtual Desktop's, set seconds
    before SteamVR, counts those seconds and one pace at most (review 1124, W23-verify B1). They are saved per role, so a separate communications device gets its own device back.
  - **SteamVR closes**: if Put back is on, each role gets its earlier device again (found by id, else by name) and is
    re-checked for 12 s. Then the saved devices are forgotten. With Put back off, the VR devices stay.
  - A device is found by **its id, else its name** (case-insensitive): reinstalling a driver gives its endpoints new
    ids.
  - `vr-audio.json` in the config folder keeps the devices before, what was set, the settled outside defaults and the
    last event. If the app quits or the PC turns off during VR, the next start (SteamVR not running) puts the devices
    back only where Windows still has the VR device. If the person changed it since, it is left alone. If the app
    restarts during VR, the saved devices from before are kept.
  - The last event reaches the window as `Snapshot::vr_audio` (`controller::vr_audio::Status`).
- `crates/orbiters-desktop/src/pages/vr_audio.rs`: the Settings section. Devices are listed off the UI thread on each
  visit and whenever SteamVR starts or stops. The test sound plays on a worker.
- Screenshot: `--only settings-vr-audio` (stand-in devices, never this computer's: `audio_devices::stand_in`).

## Tests

- `cargo test -p orbiters-desktop --bin orbiters-desktop vr_audio`: selection by id/name, setting all roles,
  re-setting after another app changes the default (and stopping after 5 times), late and missing devices, restoring
  each role, Put back off, Don't change never touching Windows, a choice made during VR, restarts during VR, leftovers
  after a crash, Windows refusing, and `vr-audio.json` round-tripping.
- `cargo test -p orbiters-desktop --bin orbiters-desktop lists_this_computers -- --ignored --nocapture`: lists this
  computer's devices and defaults, read only.

## Still to confirm in a real SteamVR session

- `SetDefaultEndpoint` really switching the defaults on this Windows 11 build. The tests never change this PC's
  devices.
- Whether 40 s is long enough for Virtual Desktop and SteamVR's own switching, and whether either keeps setting its
  device past 5 sets. Look at the subtitle: "Set again … (another app had changed it)" shows a re-set.
- That putting back after SteamVR closes is not undone by Virtual Desktop restoring its own choice later than 12 s.
- When SteamVR starts the app (Start with SteamVR) and the app never ran outside VR before, "before" is the default
  at the first set, which may already be Virtual Desktop Audio.
