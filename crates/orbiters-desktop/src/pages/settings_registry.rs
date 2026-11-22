//! Every setting of the Settings page, once (REQUEST21 2): its sub-tab, the card that draws it, the rows it is, what it
//! does and the other words people look for it by. The sub-tabs, the search and every link into Settings read this list,
//! and the page's test fails on a row drawn without an entry here, so no setting can exist without being found.
//!
//! As the Orbiters website's admin page (`frontend/src/components/elements/adminSearch`): the words typed are folded
//! (case, accents, full-width letters, katakana as hiragana: [`orbiters_core::search::fold`]), every one of them must be
//! in the setting's title, its sub-tab, its card, its description or its keywords, and a title that starts with a word
//! comes first. Texts are in the language now; the English ones match too, with less weight, for who knows a setting by
//! its English name.

use orbiters_i18n::{mark, tr, tr_ctx};
use orbiters_ui::icons;

/// A sub-tab of Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    #[default]
    Account,
    N4n0,
    Voice,
    Vr,
    Capture,
    Vrchat,
    Language,
    Privacy,
    App,
}

impl Group {
    pub const ALL: [Group; 9] = [Group::Account, Group::N4n0, Group::Voice, Group::Vr, Group::Capture, Group::Vrchat, Group::Language, Group::Privacy, Group::App];

    /// Its name in links (`settings:<id>`): never changes.
    pub fn id(self) -> &'static str {
        match self {
            Group::Account => "account",
            Group::N4n0 => "n4n0",
            Group::Voice => "voice",
            Group::Vr => "vr",
            Group::Capture => "capture",
            Group::Vrchat => "vrchat",
            Group::Language => "language",
            Group::Privacy => "privacy",
            Group::App => "app",
        }
    }

    pub fn english(self) -> &'static str {
        match self {
            Group::Account => mark("Accounts"),
            Group::N4n0 => mark("N4N0 companion"),
            Group::Voice => mark("Voice & assistant"),
            Group::Vr => mark("Overlay & VR"),
            Group::Capture => mark("Capture & storage"),
            Group::Vrchat => mark("VRChat & performance"),
            Group::Language => mark("Language & translation"),
            Group::Privacy => mark("Privacy & feedback"),
            Group::App => mark("App & updates"),
        }
    }

    pub fn title(self) -> &'static str {
        tr(self.english())
    }

    /// What is in it, under its name.
    pub fn about(self) -> &'static str {
        tr(match self {
            Group::Account => mark("Your Orbiters account, its explorer points, and your own VRChat account."),
            Group::N4n0 => mark("N4N0 on your avatar, its menu and its calls."),
            Group::Voice => mark("What N4N0 hears and how it answers: its AI, its ears, your microphones."),
            Group::Vr => mark("Orbiters around VRChat's menus and in your headset."),
            Group::Capture => mark("How captures are recorded, what they keep, and where they are stored."),
            Group::Vrchat => mark("How VRChat starts and runs, and what helps its frame rate."),
            Group::Language => mark("The language of the app, and translation in VR."),
            Group::Privacy => mark("What N4N0 shares, and the reports that help the beta."),
            Group::App => mark("The window, starting, updates, and every notice shown."),
        })
    }

    pub fn icon(self) -> &'static str {
        match self {
            Group::Account => icons::USER_CIRCLE,
            Group::N4n0 => icons::N4N0,
            Group::Voice => icons::MICROPHONE,
            Group::Vr => icons::VIRTUAL_REALITY,
            Group::Capture => icons::APERTURE,
            Group::Vrchat => icons::GAUGE,
            Group::Language => icons::GLOBE,
            Group::Privacy => icons::SHIELD_CHECK,
            Group::App => icons::INFO,
        }
    }

    pub fn of_id(id: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.id() == id)
    }
}

/// When an entry's card is drawn at all: what the page itself asks before drawing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Always,
    /// VRChat runs on this computer (Windows: Spout, its log, SteamVR).
    VrchatHere,
    /// VRChat is on the network (a Mac).
    NotVrchatHere,
    Windows,
    /// The VRChat bridge, for an account the server offers it to (on a Mac too).
    Bridge,
    /// A VRChat account can be connected (the server allows it).
    VrchatLogin,
    /// The window can choose its GPU.
    GpuChoice,
    /// The window can choose its GPU and is on the graphics card.
    GpuSplit,
}

/// What the page knows when it searches: which gates are open.
#[derive(Clone, Copy, Debug, Default)]
pub struct Open {
    pub bridge: bool,
    pub vrchat_login: bool,
    pub gpu_choice: bool,
    pub gpu_split: bool,
}

impl Open {
    pub fn lets(&self, gate: Gate) -> bool {
        match gate {
            Gate::Always => true,
            Gate::VrchatHere => orbiters_core::osc::VRCHAT_RUNS_HERE,
            Gate::NotVrchatHere => !orbiters_core::osc::VRCHAT_RUNS_HERE,
            Gate::Windows => cfg!(windows),
            Gate::Bridge => self.bridge,
            Gate::VrchatLogin => self.vrchat_login,
            Gate::GpuChoice => self.gpu_choice,
            Gate::GpuSplit => self.gpu_split,
        }
    }

    /// Everything open (the tests, which draw every card).
    pub const ALL: Open = Open { bridge: true, vrchat_login: true, gpu_choice: true, gpu_split: true };
}

/// One setting.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Its name in links (`settings:<id>`): never changes.
    pub id: &'static str,
    pub group: Group,
    /// The English title of the card that draws it.
    pub card: &'static str,
    /// The English title the search shows.
    pub title: &'static str,
    /// The English labels of the rows it is (none: its title is its one row).
    pub rows: &'static [&'static str],
    /// What it does, in English.
    pub about: &'static str,
    /// Other words for it, in English; each language has its own synonyms.
    pub keywords: &'static str,
    /// The context its title and rows are translated in (`tr_ctx`), when they have one.
    pub context: &'static str,
    pub gate: Gate,
    /// It is the whole card: a link to it lights the card.
    pub whole: bool,
    /// Its rows show only in some states (a parent switch on, signed in, a problem to tell): a link lights its card meanwhile.
    pub sometimes: bool,
}

const fn e(id: &'static str, group: Group, card: &'static str, title: &'static str, about: &'static str, keywords: &'static str) -> Entry {
    Entry { id, group, card, title, rows: &[], about, keywords, context: "", gate: Gate::Always, whole: false, sometimes: false }
}

impl Entry {
    const fn rows(self, rows: &'static [&'static str]) -> Self {
        Entry { rows, ..self }
    }
    const fn gate(self, gate: Gate) -> Self {
        Entry { gate, ..self }
    }
    const fn whole(self) -> Self {
        Entry { whole: true, ..self }
    }
    const fn sometimes(self) -> Self {
        Entry { sometimes: true, ..self }
    }
    const fn context(self, context: &'static str) -> Self {
        Entry { context, ..self }
    }

    /// A text of it in the language now.
    pub fn shown(&self, english: &'static str) -> &'static str {
        if self.context.is_empty() {
            tr(english)
        } else {
            tr_ctx(self.context, english)
        }
    }

    /// The English labels of its rows.
    pub fn row_keys(&self) -> &[&'static str] {
        if self.rows.is_empty() && !self.whole {
            std::slice::from_ref(&self.title)
        } else {
            self.rows
        }
    }

    /// Whether `label` (as drawn, in the language now) is one of its rows.
    pub fn has_row(&self, label: &str) -> bool {
        self.row_keys().iter().any(|k| self.shown(k) == label)
    }
}

use crate::controller::explorer::{EXPLORER_POINTS, NOTICES_SWITCH, NOTICES_SWITCH_WHY};
use orbiters_n4n0_core::menu::CALLS_SOON;
use Group::*;

// Cards whose titles are new here (the others keep their own).
pub const CARD_AROUND: &str = mark("Around VRChat's menus");
pub const CARD_HEADSET: &str = mark("In the headset");
pub const CARD_PRIVACY: &str = mark("Privacy");
pub const ROW_WHERE: &str = mark("Where it shows");
pub const ROW_HELD: &str = mark("Held parameters");

/// Every setting, in the order of the page.
pub static ENTRIES: &[Entry] = &[
    // Accounts.
    e("account.orbiters", Account, mark("Orbiters account"), mark("Orbiters account"), mark("One sign-in for Orbiters - N4N0 and your Unity tools: MCB, My Avatar, ReFit and the Logger. Uploads to the Map explorer use it."), mark("sign in log in login sign out website MCB My Avatar"))
        .rows(&[mark("Sign in"), mark("Connect in your browser"), mark("Shared with your Unity tools")]),
    e("account.explorer-points", Account, mark("Orbiters account"), EXPLORER_POINTS, mark("Your points and title for sharing the first public 3D pictures of worlds."), mark("points title rank score explorer")).sometimes(),
    e("account.world-notices", Account, mark("Orbiters account"), NOTICES_SWITCH, NOTICES_SWITCH_WHY, mark("notice prompt reminder world 3D picture")),
    e("account.vrchat", Account, mark("VRChat account"), mark("VRChat account"), mark("Instead of Orbiters' shared account: friends, instances and pictures come from yours."), mark("login password two-factor 2FA code connect friends instances"))
        .rows(&[mark("Username"), mark("Password"), mark("Authenticator code"), mark("Email code"), mark("Recovery code")])
        .gate(Gate::VrchatLogin)
        .whole()
        .sometimes(),
    // N4N0 companion.
    e("n4n0.enabled", N4n0, "N4N0", "N4N0", mark("The panel, the AFK message, carried and held parameters, saved videos."), mark("on off enable disable companion bot")),
    e("n4n0.show-bot", N4n0, "N4N0", mark("Show N4N0"), mark("The bot beside your head, its beams and its panel, for everyone around. Off, N4N0's menu still opens, before your eyes as ever. Show N4N0 in your Action Menu does the same."), mark("bot hide visible beams")),
    e("n4n0.afk", N4n0, "N4N0", mark("AFK message"), mark("While VRChat says you are away, the panel tells people around you."), mark("away AFK status message text")).rows(&[mark("AFK message"), mark("Message")]),
    e("n4n0.keep-pose", N4n0, "N4N0", mark("Keep my pose"), mark("GoGoLoco's pose, height and float carry over to the next avatar you wear."), mark("GoGoLoco height float carry avatar change")),
    e("n4n0.controller-input", N4n0, "N4N0", mark("Controller input"), mark("Your touchpads and sticks reach avatars made to use N4N0's input parameters (Orbiters Toolkit's README tells creators how)."), mark("touchpad stick joystick parameters")),
    e("n4n0.panel-speed", N4n0, "N4N0", mark("Panel speed"), mark("Time between two blocks of text. Faster than 250 ms, people around you miss blocks."), mark("text speed delay")),
    e("n4n0.held", N4n0, "N4N0", ROW_HELD, mark("Parameters N4N0 puts back whenever VRChat or a world resets them."), mark("pin hold parameter let go")),
    e("n4n0.menu-where", N4n0, mark("N4N0's menu"), ROW_WHERE, mark("N4N0's own menu, opened by its dedicated button or the watch on your wrist, shows on your avatar's N4N0 screen when it has one, and floats before you when it does not."), mark("avatar screen floating display")).gate(Gate::VrchatHere),
    e("n4n0.menu-button", N4n0, mark("N4N0's menu"), mark("Controller button"), mark("SteamVR's \"Open N4N0's menu\" binding for Orbiters: two quick taps on the left thumbrest or trackpad (a double press of X on controllers with neither), unless you change it in SteamVR's controller settings. The watch on your wrist opens it too."), mark("thumbrest trackpad double tap binding SteamVR open")),
    e("n4n0.menu-wake", N4n0, mark("N4N0's menu"), mark("Say \"Nano\""), mark("The wake word on its own opens the menu."), mark("voice wake word open")),
    e("n4n0.menu-touch", N4n0, mark("N4N0's menu"), mark("Touch N4N0"), mark("A hand on N4N0 opens the menu; touching it again closes it."), mark("hand touch open")),
    e("n4n0.menu-try", N4n0, mark("N4N0's menu"), mark("Try it"), mark("Opens the menu (or closes it) as the button and N4N0 do."), mark("open test")),
    e("n4n0.keyboard", N4n0, mark("N4N0's menu"), mark("Keyboard"), mark("The letters of the search keyboard. Automatic follows this computer's keyboard."), mark("QWERTY AZERTY layout letters")),
    e("n4n0.look", N4n0, mark("N4N0's menu"), "Look", mark("Retro: a green phosphor terminal, as on an old computer. N4N0's screen has its own Look, in My Avatar."), mark("retro modern theme style appearance")).context("appearance"), // i18n: not shown ("Look" is translated in its context, tr_ctx("appearance", "Look"))
    e("n4n0.screen-size", N4n0, mark("N4N0's menu"), mark("Screen size"), mark("How large N4N0's projected screen is, for you and everyone around."), mark("small large bigger size")),
    e("n4n0.calls", N4n0, mark("Calls"), mark("Calls"), mark("Your presence and your VRChat friends (as hashes, from VRCX and VRCNext) go to Orbiters, to find friends you can call. Calls go through Orbiters' relay: a friend never sees this computer's address."), mark("call voice friends relay phone")).rows(&[mark("Calls"), CALLS_SOON]),
    e("n4n0.direct-calls", N4n0, mark("Calls"), mark("Direct calls"), mark("Connect straight to the friend when your networks allow it: a little faster, but each of you sees the other's IP address."), mark("IP address peer direct faster")).sometimes(),
    // Voice & assistant.
    e("voice.assistant", Voice, mark("Assistant"), mark("AI assistant"), mark("Off, N4N0 is a tool without AI: nothing is transcribed, and nothing goes to Orbiters."), mark("AI brain chat answers")),
    e("voice.personality", Voice, mark("Assistant"), mark("Personality and own key"), mark("N4N0's roast level, its language and your own AI key are on the Orbiters website."), mark("roast API key website")),
    e("voice.models", Voice, mark("N4N0's ears"), mark("Speech models"), mark("Speech to text on this computer, in 25 languages (NVIDIA Parakeet, Silero VAD): about 490 MB to download. Nothing you say leaves it until it is a sentence for N4N0."), mark("speech recognition transcription install download")),
    e("voice.mic", Voice, mark("N4N0's ears"), mark("Microphone"), mark("The one N4N0 listens to."), mark("mic input device test")),
    e("voice.wake", Voice, mark("N4N0's ears"), mark("Wake word"), mark("Start a sentence with “Nano” (N4N0 said aloud) and N4N0 hears it, any time. Every sentence is transcribed on this computer to find it; none is kept."), mark("Nano listen voice hands-free")),
    e("voice.mute", Voice, mark("N4N0's ears"), mark("Mute VRChat while I hold Talk"), mark("The sentence for N4N0 stays out of the world."), mark("mute talk push to talk")),
    e("voice.windows-mics", Voice, mark("Microphones"), mark("Microphones"), mark("Speak: the one that moves is the one hearing you. Press one to make it Windows' default."), mark("default mic input level Windows")).gate(Gate::Windows).whole(),
    // Overlay & VR.
    e("vr.wheel", Vr, CARD_AROUND, mark("Around the Action Menu's wheel"), mark("Orbiters around VRChat's own wheel, with it, whatever N4N0's menu does: opened with R in desktop mode, or by holding the menu button in VR."), mark("Action Menu wheel ring radial overlay")).gate(Gate::VrchatHere),
    e("vr.camera", Vr, CARD_AROUND, mark("Around the camera"), mark("3D pictures, Record and the QR code beside VRChat's camera. In a headset they appear where VRChat opens it and move with the hand that holds it; in desktop mode, at the left of its panel."), mark("camera tiles 3D picture QR record")).gate(Gate::VrchatHere),
    e("vr.camera-find", Vr, CARD_AROUND, mark("Find the camera on screen"), mark("Also looks for the camera's panel in what VRChat shows and moves the tiles onto it. Off by default: it can put them in the wrong place. It keeps nothing it sees."), mark("detect pattern camera panel")).gate(Gate::VrchatHere).sometimes(),
    e("vr.camera-now", Vr, CARD_AROUND, mark("The camera's tiles, right now"), "", "").gate(Gate::VrchatHere).sometimes(),
    e("vr.turn", Vr, CARD_AROUND, mark("Turn around in a 3D capture"), mark("In a headset, with a capture around you: VRChat's turning stick turns what you see, smoothly or in steps as VRChat is set to."), mark("turning stick snap smooth rotate")).gate(Gate::VrchatHere),
    e("vr.quick-menu", Vr, CARD_AROUND, mark("Around the Quick Menu"), mark("Orbiters beside VRChat's own Quick Menu, with it, whatever N4N0's menu does: opened with Esc in desktop mode, or by a short press of the menu button in VR."), mark("Quick Menu side tiles Esc")).gate(Gate::VrchatHere),
    e("vr.size", Vr, CARD_AROUND, mark("Panels' size in VR"), mark("If they sit too near or too far from VRChat's menu in your headset."), mark("scale bigger smaller distance")).gate(Gate::VrchatHere),
    e("vr.motion", Vr, CARD_AROUND, mark("How they follow your hand in VR"), mark("The tiles around VRChat's menus each follow your hand on a little spring of their own."), mark("spring rigid follow hand")).gate(Gate::VrchatHere),
    e("vr.smoothing-quick-menu", Vr, CARD_AROUND, mark("Smoothing on the Quick Menu"), mark("How softly what is rigid follows your hand: what opens in front of VRChat's menu, always, and the tiles when they match it. About 35 ms lines up with VRChat."), mark("latency smooth jitter")).gate(Gate::VrchatHere),
    e("vr.smoothing-wheel", Vr, CARD_AROUND, mark("Smoothing on the wheel"), mark("The same around VRChat's Action Menu. About 20 ms lines up with VRChat."), mark("latency smooth jitter")).gate(Gate::VrchatHere),
    e("vr.smoothing-reset", Vr, CARD_AROUND, mark("Reset the smoothing"), mark("Puts both smoothings back to the values measured to match VRChat"), mark("default measured smoothing back")).gate(Gate::VrchatHere),
    e("vr.right-now", Vr, CARD_AROUND, mark("Right now"), "", "").gate(Gate::VrchatHere).sometimes(),
    e("vr.dashboard", Vr, CARD_HEADSET, mark("SteamVR dashboard tab"), mark("Orbiters as a tab in SteamVR's dashboard while SteamVR runs."), mark("SteamVR overlay dashboard tab")).gate(Gate::VrchatHere),
    e("vr.watch", Vr, CARD_HEADSET, mark("Orbiters watch"), mark("The N4N0 mark on a wrist in VR, shown as you turn the wrist to look at it. Point at it with the other hand and pull the trigger once: N4N0's menu opens before you (or on your avatar's N4N0 screen), used with your controllers' lasers while VRChat keeps them; pull again to close it. Grip it to move it. The Orbiters panel floats beside you from the SteamVR tab's Float button."), mark("wrist watch hand left right"))
        .rows(&[mark("Orbiters watch"), mark("Watch on")])
        .gate(Gate::VrchatHere),
    e("vr.coverage", Vr, CARD_HEADSET, mark("Coverage while recording"), mark("In VR, the camera path and what it has covered float above a controller while you record, as in the capture options of Photos & captures. SteamVR shows it in your headset only: never in VRChat's camera or the recording."), mark("path coverage controller")).gate(Gate::VrchatHere),
    e("vr.walk", Vr, CARD_HEADSET, mark("Walk boost"), mark("In VR, your walking stick carries you further, so you walk faster in VRChat. Also in the SteamVR tab and N4N0's menu."), mark("fast walk speed strength locomotion OVR Advanced Settings driver"))
        .rows(&[mark("Walk boost"), mark("Strength"), mark("Through SteamVR's driver"), mark("Fast walk's lesser mode")])
        .gate(Gate::VrchatHere),
    e("vr.audio-output", Vr, mark("When SteamVR starts"), mark("Output in VR"), mark("The speakers or headphones Windows makes every app's default when SteamVR starts, set again over its first seconds as Virtual Desktop and SteamVR put their own."), mark("headphones speakers sound audio device")).gate(Gate::VrchatHere),
    e("vr.audio-input", Vr, mark("When SteamVR starts"), mark("Input in VR"), "", mark("microphone mic sound audio device")).gate(Gate::VrchatHere),
    e("vr.audio-restore", Vr, mark("When SteamVR starts"), mark("Put back the devices before"), mark("When SteamVR closes, the output and input Windows had before it started come back."), mark("restore sound audio device")).gate(Gate::VrchatHere),
    // Capture & storage.
    e("capture.kf-distance", Capture, mark("Capture"), mark("Keyframe distance"), mark("A new lossless keyframe after the camera moves this far."), mark("keyframe meters")).gate(Gate::VrchatHere),
    e("capture.kf-turn", Capture, mark("Capture"), mark("Keyframe turn"), mark("…or after it turns this much."), mark("keyframe degrees rotation")).gate(Gate::VrchatHere),
    e("capture.kf-interval", Capture, mark("Capture"), mark("Keyframe interval"), mark("…or after this long, whatever happens."), mark("keyframe seconds time")).gate(Gate::VrchatHere),
    e("capture.split", Capture, mark("Capture"), mark("New session on world change"), mark("Each VRChat world gets its own session while you record."), mark("split session world")).gate(Gate::VrchatHere),
    e("capture.spout", Capture, mark("Capture"), mark("Spout sender"), mark("Leave empty to pick VRChat's Stream camera automatically."), mark("Spout source stream video input")).gate(Gate::VrchatHere),
    e("capture.reconstruct-while", Capture, mark("Capture"), mark("Reconstruct while VRChat runs"), mark("On the graphics card training gives way to VRChat's frames; the processor's integrated GPU leaves the game alone but is about 40 times slower."), mark("reconstruction GPU graphics card integrated training splats")).gate(Gate::VrchatHere),
    e("capture.notice", Capture, mark("Capture"), mark("Capture notice"), mark("The consent reminder shown before the first capture."), mark("consent reminder show again")),
    e("capture.osc", Capture, mark("VRChat and OSC"), mark("Action Menu control"), mark("Start and stop recording with N4N0 › Record in your avatar's Action Menu, and let N4N0 and its capture kit hear Orbiters (OSCQuery, works next to VRCFT and other OSC apps)."), mark("OSC OSCQuery port record Action Menu")),
    e("capture.stream-camera", Capture, mark("VRChat and OSC"), mark("Stream camera"), mark("While recording, open VRChat's camera in Stream mode with Spout Stream on (the picture comes from it), then put it back."), mark("camera Spout stream mode")).gate(Gate::VrchatHere),
    e("capture.rig-views", Capture, mark("VRChat and OSC"), mark("Six more views while recording"), mark("This often, N4N0's camera rig takes six views around your camera (left, front, right, back, up, down) wherever it has not yet, so the reconstruction sees what you did not film. VRChat draws the world six more times for those moments; 3D pictures and scans take theirs all along."), mark("rig views cube coverage")).gate(Gate::VrchatHere),
    e("capture.dolly", Capture, mark("VRChat and OSC"), mark("Camera dolly streams (experimental)"), mark("VRC+: a 3D picture flies four cameras with VRChat's camera dolly, one left, one right and one above yours, all looking where yours does, and recordings also read the dolly's other streams. In VRChat, set Camera › Dolly › Streaming to Multi once."), mark("dolly VRC+ multi stream cameras")).gate(Gate::VrchatHere),
    e("capture.log", Capture, mark("VRChat and OSC"), mark("VRChat log"), mark("Found, waiting for VRChat"), mark("log file output")).gate(Gate::VrchatHere),
    e("capture.parameters", Capture, mark("Performance"), mark("Face and avatar parameters"), mark("Every parameter VRChat sends over OSC: face tracking from VRCFaceTracking, gestures, visemes, movement."), mark("face tracking VRCFT gestures visemes")).gate(Gate::VrchatHere),
    e("capture.voice", Capture, mark("Performance"), mark("Your voice"), mark("The microphone, for your avatar's visemes in Mixer."), mark("microphone mic record audio")).gate(Gate::VrchatHere),
    e("capture.game-sound", Capture, mark("Performance"), mark("VRChat sound"), mark("What VRChat plays, world sounds and other players' voices; nothing else on the PC. Ask the people around you first."), mark("game audio sound record")).gate(Gate::VrchatHere),
    e("capture.tracking", Capture, mark("Performance"), mark("Body tracking"), mark("Headset, controllers and trackers from SteamVR, full-body estimation included."), mark("full body FBT trackers SteamVR")).gate(Gate::VrchatHere),
    e("capture.tracking-rate", Capture, mark("Performance"), mark("Tracking rate"), mark("Device poses recorded per second."), mark("Hz frequency poses")).gate(Gate::VrchatHere),
    e("storage.free", Capture, mark("Storage"), mark("Free up space"), mark("What the captures and 3D pictures take, and deleting what they no longer need."), mark("disk space delete clean")),
    e("storage.limit", Capture, mark("Storage"), mark("Storage limit"), mark("Recording stops when the library reaches it (0 = no limit)."), mark("disk space GB quota library")),
    e("storage.folder", Capture, mark("Storage"), mark("Captures folder"), "", mark("folder path directory location drive")),
    e("storage.raw-folder", Capture, mark("Storage"), mark("Raw captures folder"), mark("Where Mixer's capture simulator writes."), mark("folder path directory Mixer")),
    // VRChat & performance.
    e("vrchat.booster", Vrchat, mark("FPS booster"), mark("FPS booster"), mark("Spend what helps your frames. Shows what it really gains."), mark("FPS frame rate lag boost faster")).gate(Gate::VrchatHere).whole(),
    e("vrchat.launch-options", Vrchat, mark("Play VRChat"), mark("Performance launch options"), mark("VRChat gets a little more of the processor, a frame cap, video decoding on the graphics card and, in VR, a tiny desktop window. Helps when the processor holds VRChat back."), mark("launch start Play options CPU")).gate(Gate::VrchatHere),
    e("vrchat.priority", Vrchat, mark("Play VRChat"), mark("VRChat's priority"), mark("Above normal is enough. High can take time from SteamVR and Virtual Desktop and make the headset stutter."), mark("CPU process priority")).gate(Gate::VrchatHere).sometimes(),
    e("vrchat.main-thread", Vrchat, mark("Play VRChat"), mark("Main thread priority"), mark("The thread VRChat's world runs on."), mark("CPU thread priority")).gate(Gate::VrchatHere).sometimes(),
    e("vrchat.frame-cap", Vrchat, mark("Play VRChat"), mark("Frame cap"), mark("Frames per second on the desktop. In VR, the headset sets the pace."), mark("FPS limit cap")).gate(Gate::VrchatHere).sometimes(),
    e("vrchat.video-decoding", Vrchat, mark("Play VRChat"), mark("Video decoding on the graphics card"), mark("Videos in worlds cost the processor less."), mark("video GPU decoding hardware")).gate(Gate::VrchatHere).sometimes(),
    e("vrchat.tiny-window", Vrchat, mark("Play VRChat"), mark("Tiny desktop window in VR"), mark("A 100 × 100 window on the monitor while you play in VR, so it costs almost nothing. Turn it off to show or record the game on the monitor. Desktop mode keeps its window."), mark("window monitor desktop small")).gate(Gate::VrchatHere).sometimes(),
    e("vrchat.perf-report", Vrchat, mark("Performance report"), mark("Performance report"), mark("If VRChat or your computer lags with N4N0, this puts together what N4N0 measured over the last hour with your computer's specifications and the settings that matter. You see it before anything is sent, and it is kept on this computer."), mark("lag slow report diagnostics")).gate(Gate::VrchatHere).whole(),
    e("vrchat.bridge", Vrchat, mark("Experimental"), mark("VRChat bridge"), mark("Lets an agent such as Claude Code start VRChat in desktop mode, go from private instance to private instance, look around and take 3D pictures, through a service on this computer only. Your own keyboard or mouse pauses it."), mark("agent MCP automation bridge"))
        .rows(&[mark("VRChat bridge"), mark("Screen recording"), mark("It stays closed"), mark("Address"), mark("Token"), mark("Taking over")])
        .gate(Gate::Bridge),
    e("vrchat.network", Vrchat, mark("VRChat on your network"), mark("VRChat on your network"), mark("When it is not found by itself (some routers keep devices from finding each other)."), mark("network LAN address headset phone PC"))
        .rows(&[mark("Allow the local network"), mark("Address you typed"), mark("VRChat on a headset or a phone"), mark("VRChat on another PC"), mark("Enter its address")])
        .gate(Gate::NotVrchatHere)
        .whole(),
    // Language & translation.
    e("language.app", Language, mark("Language"), mark("Language"), mark("Saved to your Orbiters account, and used on the website too."), mark("English French Japanese Spanish Portuguese")),
    e("translate.on", Language, mark("Translation in VR"), mark("Translation"), mark("Reads the text your headset shows, a few times a second. Off, nothing of it runs."), mark("translate signs chat")).gate(Gate::VrchatHere),
    e("translate.into", Language, mark("Translation in VR"), mark("Into"), mark("The language translations are written in."), mark("target language")).gate(Gate::VrchatHere),
    e("translate.provider", Language, mark("Translation in VR"), mark("Translator"), mark("Who translates what is read."), mark("provider AI server LM Studio")).gate(Gate::VrchatHere),
    e("translate.server", Language, mark("Translation in VR"), mark("Address"), mark("An OpenAI-compatible server on your network, such as LM Studio's."), mark("server model LM Studio AI")).rows(&[mark("Address"), mark("Model")]).gate(Gate::VrchatHere).sometimes(),
    e("translate.english", Language, mark("Translation in VR"), mark("English"), mark("VRChat's menus and most names are in English."), mark("translate English text")).gate(Gate::VrchatHere),
    // Privacy & feedback.
    e("privacy.world-performance", Privacy, CARD_PRIVACY, mark("Share my frame rate per world"), mark("In VR, a summary a minute: the world, your FPS, your avatar, how many people were there and the names of their avatars, never who they are, plus your graphics card, processor and headset. It shows which worlds run well on the Map explorer."), mark("FPS statistics share world Map explorer")).gate(Gate::VrchatHere),
    e("privacy.usage", Privacy, CARD_PRIVACY, mark("Beta usage statistics"), mark("Counts which pages and buttons of Orbiters - N4N0 you use, in what order and when, and every few seconds how much processor, graphics card and memory the app, VRChat and the whole computer use, VRChat's frame rate in VR, and the basic hardware (processor and graphics card models, memory size, headset and its refresh rate). When you are signed in, it is linked to your Orbiters account and deleted with it. Only the Orbiters team sees it, to make the app easier and lighter; it is never shared publicly. Never what you type or say, pictures, links, worlds or people. Turned off, nothing more is counted and what was waiting to be sent is deleted."), mark("telemetry analytics statistics data tracking")).gate(Gate::VrchatHere),
    e("privacy.mismatch", Privacy, mark("Beta feedback"), mark("Menu mismatch buttons"), mark("Shows three buttons, always in view: \"Menu should be off\", \"Wheel should be on\" and \"Quick menu should be on\", in VR on your watch's wrist, on the desktop in the bottom left corner of VRChat's window. Press one when N4N0's menus do not match VRChat's. Each press keeps a report on this computer: the controller buttons and keys used for the menus and what N4N0 decided in the 30 seconds before, and VRChat's log lines about menus and worlds. Never what you type or say, nor anyone's name. Turned off, nothing is recorded."), mark("bug report feedback beta")).gate(Gate::VrchatHere),
    e("privacy.mismatch-picture", Privacy, mark("Beta feedback"), mark("Add a picture of the game"), mark("A small, blurred picture of what VRChat showed at the press goes with the report: the best clue of what its menus did, but it is a picture of your game, and can show the world and the people in it."), mark("screenshot picture report")).gate(Gate::VrchatHere),
    // App & updates.
    e("app.reduce-motion", App, mark("Interface"), mark("Reduce motion"), mark("Calmer transitions and a still background."), mark("animations accessibility calm")),
    e("app.close-minimized", App, mark("Interface"), crate::window_rest::SETTING, crate::window_rest::SETTING_HINT, mark("tray memory minimized close")).gate(Gate::VrchatHere),
    e("app.window-gpu", App, mark("Interface"), mark("Window graphics"), mark("The graphics card draws the window for less than it costs to copy it there from the integrated GPU."), mark("GPU graphics card integrated window")).gate(Gate::GpuChoice),
    e("app.view-gpu", App, mark("Interface"), mark("3D view on the integrated GPU"), mark("The 3D view's splats draw on the integrated GPU and only their pictures reach the graphics card: orbiting costs a game little, at about half the frame rate."), mark("GPU integrated splats 3D view")).gate(Gate::GpuSplit),
    e("app.start-windows", App, mark("Start"), mark("Start with Windows"), mark("Waits in the tray, ready when VRChat starts."), mark("startup boot login autostart tray")).rows(&[mark("Start with Windows"), mark("Start at login")]),
    e("app.start-steamvr", App, mark("Start"), mark("Start with SteamVR"), mark("Opens when SteamVR starts, in its dashboard (SteamVR lists it with its startup overlay apps)."), mark("startup autostart SteamVR overlay")).gate(Gate::VrchatHere),
    e("app.about", App, mark("About"), mark("About"), mark("Copyright © 2026 Enzo DUTRA / blackorbit. Free to use; see the licence for what is allowed."), mark("version licence worker log quit")).whole(),
    e("app.updates", App, mark("About"), mark("Updates"), mark("Orbiters - N4N0 looks for a newer version shortly after it starts, then every few hours."), mark("update version upgrade new release restart")),
    e("app.development", App, mark("About"), mark("Development environment"), mark("For testing development builds: the Orbiters server running on this computer instead of orbiters.cc, with its own account and updates."), mark("dev server testing builds")),
    e("app.log", App, mark("Log"), mark("Log"), mark("Every notice, warning and error N4N0 showed, kept here"), mark("notices errors warnings history messages")).whole(),
];

/// The entry with this id.
pub fn entry(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.id == id)
}

/// The entries of a sub-tab that can show here.
pub fn of_group(group: Group, open: &Open) -> impl Iterator<Item = &'static Entry> + '_ {
    ENTRIES.iter().filter(move |e| e.group == group && open.lets(e.gate))
}

/// The sub-tabs that have something to show here.
pub fn groups(open: &Open) -> Vec<Group> {
    Group::ALL.into_iter().filter(|g| of_group(*g, open).next().is_some()).collect()
}

/// The words of a search: folded, each once, at most eight (as the website's).
pub fn words(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in orbiters_core::search::fold(query).split_whitespace() {
        if !out.iter().any(|o| o == w) {
            out.push(w.to_owned());
        }
        if out.len() == 8 {
            break;
        }
    }
    out
}

/// A search result: the entry, how well it matched, and whether a word was found only in its keywords.
#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub entry: &'static Entry,
    pub score: u32,
    pub by_keyword: bool,
}

/// Every entry that matches all the words of `query`, best first (the website's `searchAdminSettings`).
pub fn search(query: &str, open: &Open) -> Vec<Found> {
    let words = words(query);
    if words.is_empty() {
        return Vec::new();
    }
    let fold = orbiters_core::search::fold;
    let english_too = orbiters_i18n::language() != orbiters_i18n::Language::En;
    let mut found: Vec<Found> = ENTRIES
        .iter()
        .filter(|e| open.lets(e.gate))
        .filter_map(|e| {
            let title = fold(e.shown(e.title));
            let place = format!("{} {}", fold(e.group.title()), fold(e.shown_card()));
            let about = fold(tr(e.about));
            let keywords = if e.keywords.is_empty() { String::new() } else { fold(tr(e.keywords)) };
            let english = if english_too { fold(&format!("{} {} {} {}", e.title, e.card, e.group.english(), e.keywords)) } else { String::new() };
            let mut score = 0u32;
            let mut by_keyword = false;
            for w in &words {
                let w = w.as_str();
                let seen = title.contains(w) || place.contains(w) || about.contains(w);
                if !seen && !keywords.contains(w) && !english.contains(w) {
                    return None;
                }
                by_keyword |= !seen;
                score += if title == w {
                    100
                } else if title.starts_with(w) {
                    35
                } else if title.split(' ').any(|t| t.starts_with(w)) {
                    25
                } else if title.contains(w) {
                    20
                } else {
                    0
                };
                score += if place.contains(w) { 8 } else { 0 } + if about.contains(w) { 4 } else { 0 } + if keywords.contains(w) { 3 } else { 0 } + u32::from(english.contains(w));
            }
            Some(Found { entry: e, score, by_keyword })
        })
        .collect();
    found.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.entry.shown(a.entry.title).cmp(b.entry.shown(b.entry.title))));
    found
}

impl Entry {
    /// Its card's title in the language now.
    pub fn shown_card(&self) -> &'static str {
        tr(self.card)
    }
}

/// The parts of `text` the words of a search are in, as byte ranges (the website's `highlightParts`): each character
/// is folded alone, so the marks land on the text as written, accents and all.
pub fn marks(text: &str, words: &[String]) -> Vec<std::ops::Range<usize>> {
    let mut folded = String::new();
    // For each byte of `folded`, the character of `text` it came from.
    let mut from: Vec<std::ops::Range<usize>> = Vec::new();
    for (at, c) in text.char_indices() {
        let mut buf = [0u8; 4];
        let one = orbiters_core::search::fold(c.encode_utf8(&mut buf));
        let one = if one.is_empty() && c.is_whitespace() { " ".to_owned() } else { one };
        for _ in 0..one.len() {
            from.push(at..at + c.len_utf8());
        }
        folded.push_str(&one);
    }
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for w in words {
        let mut start = 0;
        while let Some(i) = folded[start..].find(w.as_str()) {
            let i = start + i;
            let end = i + w.len();
            ranges.push(from[i].start..from[end - 1].end);
            start = end;
        }
    }
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_i18n::{in_language, Language};

    fn ids(query: &str) -> Vec<&'static str> {
        search(query, &Open::ALL).iter().map(|f| f.entry.id).collect()
    }

    #[test]
    fn every_id_is_once_and_every_group_has_entries() {
        let mut seen = std::collections::BTreeSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.id), "{} twice", e.id);
            assert!(e.id.starts_with(e.group.id()) || e.id.starts_with("storage.") || e.id.starts_with("translate."), "{} is not named after its group {:?}", e.id, e.group);
            assert!(!e.row_keys().is_empty() || e.whole, "{} has no row", e.id);
        }
        for g in Group::ALL {
            assert!(of_group(g, &Open::ALL).next().is_some(), "{g:?} is empty");
            assert_eq!(Group::of_id(g.id()), Some(g));
        }
    }

    #[test]
    fn a_title_that_starts_with_the_word_comes_first() {
        let found = ids("wake");
        assert_eq!(found.first(), Some(&"voice.wake"), "{found:?}");
        assert!(found.contains(&"n4n0.menu-wake"), "its description says wake word: {found:?}");
        assert_eq!(ids("storage limit").first(), Some(&"storage.limit"));
        assert!(ids("").is_empty() && ids("   ").is_empty());
        assert!(ids("zzzz nothing like it").is_empty());
    }

    #[test]
    fn every_word_must_match_somewhere() {
        // "microphone" alone finds several; with "wake" only what has both.
        assert!(ids("microphone").len() > 2);
        let both = ids("microphone ears");
        assert!(both.iter().all(|id| id.starts_with("voice.")), "{both:?}");
    }

    #[test]
    fn keywords_and_the_group_find_a_setting_too() {
        // "telemetry" is in no title nor description: a keyword.
        let found = search("telemetry", &Open::ALL);
        assert_eq!(found.first().map(|f| f.entry.id), Some("privacy.usage"));
        assert!(found[0].by_keyword);
        // The sub-tab's name.
        assert!(ids("overlay").contains(&"vr.wheel"));
    }

    #[test]
    fn case_and_accents_do_not_matter_in_any_language() {
        in_language(Language::Fr, || {
            // "Réduire les animations" (Reduce motion), typed without accents and in capitals.
            let fr = tr("Reduce motion");
            assert_ne!(fr, "Reduce motion", "French has it");
            let typed: String = orbiters_core::search::fold(fr).to_uppercase();
            assert!(ids(&typed).contains(&"app.reduce-motion"), "{typed}: {:?}", ids(&typed));
            // A French synonym, with and without its accent.
            assert!(ids("démarrage").contains(&"app.start-windows"), "{:?}", ids("démarrage"));
            assert!(ids("demarrage").contains(&"app.start-windows"));
            // The English name still finds it.
            assert!(ids("reduce motion").contains(&"app.reduce-motion"));
        });
        in_language(Language::Ja, || {
            let ja = tr("Wake word");
            assert_ne!(ja, "Wake word");
            assert_eq!(ids(ja).first(), Some(&"voice.wake"), "{ja}");
            // Katakana and hiragana are one.
            assert!(ids("マイク").iter().any(|id| id.starts_with("voice.")), "{:?}", ids("マイク"));
        });
        for language in [Language::Es, Language::PtBr] {
            in_language(language, || {
                let title = tr("Storage limit");
                assert_ne!(title, "Storage limit", "{language:?}");
                assert!(ids(&title.to_lowercase()).contains(&"storage.limit"), "{language:?}: {title}");
            });
        }
    }

    #[test]
    fn marks_land_on_the_text_as_written() {
        let w = words("demarrage");
        let text = "Démarrage et fenêtre";
        let m = marks(text, &w);
        assert_eq!(m.len(), 1);
        assert_eq!(&text[m[0].clone()], "Démarrage");
        let m = marks("Wake word, Wake", &words("wake"));
        assert_eq!(m.len(), 2);
        assert!(marks("abc", &words("zz")).is_empty());
    }

    /// Titles, rows and descriptions are held to orbiters-i18n's coverage test (some stay as they are in a language,
    /// as product names do); the words added for the search are each language's own.
    #[test]
    fn every_entry_is_translated() {
        for language in Language::TRANSLATED {
            in_language(language, || {
                for e in ENTRIES.iter().filter(|e| !e.keywords.is_empty()) {
                    assert_ne!(tr(e.keywords), e.keywords, "{language:?}: {} has no keywords of its own", e.id);
                }
                for g in Group::ALL {
                    assert_ne!(g.title(), g.english(), "{language:?}: {g:?}");
                }
            });
        }
    }
}
