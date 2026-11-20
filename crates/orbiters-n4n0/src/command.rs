//! What the apps' interfaces ask of N4N0: the window and the SteamVR overlay over the desktop's link, a phone's
//! screen in its own process.

use crate::status::{N4n0Face, StateAction};
use serde::{Deserialize, Serialize};

/// Which of N4N0's menus asks for the QR code scanner or the friends list (MENU-13): the SteamVR menu, the panels around
/// VRChat's menus, the window's Home, the avatar's screen. `Any`: a sender that does not say (an older one), whose stop
/// ends the scan for every menu as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuHolder {
    #[default]
    Any,
    Overlay,
    Panels,
    Window,
    Screen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Page-scoped Ask lifecycle. Enter defaults to Auto; leaving cancels and releases capture.
    AskPage { open: bool },
    /// First user Talk press takes over this visit; `down` holds/releases the manual sentence.
    AskTalk { down: bool },
    /// Audio output owns this gate until playback drains, so the answer cannot become input.
    AskPlayback { active: bool },
    /// The "Talk with N4N0?" button the wake word put up during a call: pressed (`accept`), N4N0 listens; else it goes.
    WakeOffer {
        accept: bool,
        /// The offer shown (`status::WakeOffer::id`): a late press of an older card answers nothing.
        #[serde(default)]
        id: Option<u64>,
    },
    /// N4N0's assistant switched on or off from a menu (Settings' switch, the overlay's tiles, Home's microphone): off, it stops
    /// at once whatever it was doing (listening, the microphone, an answer under way).
    Assistant { on: bool },
    /// Explicit retry after a microphone/worker failure; rechecks every preflight requirement.
    AskRetry,
    /// A question typed in the window or the overlay.
    Ask {
        text: String,
    },
    /// The window's or the overlay's Talk button, pressed or let go.
    Talk {
        down: bool,
    },
    /// Shows a text on the panel as it is (tries the panel).
    Say {
        text: String,
    },
    HidePanel,
    /// Starts a new conversation (N4N0 forgets the current one).
    NewConversation,
    SaveVideo,
    RemoveVideo {
        url: String,
    },
    /// A picture with VRChat's camera after `delay_seconds`.
    Photo {
        delay_seconds: u32,
    },
    CopyText {
        text: String,
    },
    /// Opens a page of the Orbiters website (an approval).
    OpenLink {
        url: String,
    },
    /// Downloads the speech models.
    InstallVoice,
    /// Stops the speech models' download (the worker removes what it unpacked).
    CancelVoiceInstall,
    /// A 3D picture with VRChat's camera where it stands (recorded, then reconstructed).
    Photo3D,
    /// A 3D picture of `mode` with VRChat's camera where it stands, which is also the mode the menus' one button takes next.
    Photo3DOf {
        mode: orbiters_core::settings::PictureMode,
    },
    /// Calls a friend (a contact id from the calls status).
    Call {
        contact: String,
    },
    /// Answers the ringing call.
    AnswerCall,
    /// Declines the ringing call or hangs up.
    EndCall,
    MuteCall {
        on: bool,
    },
    /// Testing: holds N4N0 in a state (`None`: back to what N4N0 is doing).
    TestFace {
        face: Option<N4n0Face>,
    },
    /// Testing: shows every state in turn, two seconds each.
    TestCycle,
    /// Testing: acts as if VRChat said the owner is away (the sleeping N4N0 and the AFK message).
    TestAfk {
        on: bool,
    },
    /// Testing: acts as if the owner touched N4N0.
    TestTouch,
    /// Shows or hides the bot (`N4N0/Visible`, Show N4N0 in the avatar's Action Menu); from screen version 17 its menu
    /// stays another choice.
    ShowBot {
        on: bool,
    },
    /// Testing: the avatar's screen shows the full pictures' diagnostics (contracts `Docs/n4n0-picture.md`, Diagnostics).
    TestPictures {
        on: bool,
    },
    /// The overlay's N4N0 menu opened or closed, and whether a sentence N4N0 hears is what to search for (its search
    /// page is up). Opening it makes N4N0 listen.
    MenuState {
        open: bool,
        dictation: bool,
        /// Its keyboard shows: the computer's keyboard types there.
        #[serde(default)]
        keyboard: bool,
    },
    /// The SteamVR button for N4N0's menu while the avatar has N4N0's screen: opens the screen, or closes it.
    ToggleScreen,
    /// A 3D picture after `seconds` (N4N0's menu's timer).
    #[serde(rename = "photo_3d_in")]
    Photo3DIn {
        seconds: u32,
    },
    /// A 3D picture after `seconds` from when VRChat's camera is ready: pressed again while it waits, it is called off.
    #[serde(rename = "photo_3d_when_camera")]
    Photo3DWhenCamera {
        seconds: u32,
    },
    /// N4N0's light as N4N0's menu set it (its Light page): `N4N0/Light/On`, its colour and level.
    Light {
        on: bool,
        color: u8,
        level: f32,
    },
    /// The avatar's saved states: save, apply, the default, rename, delete, what one puts back. `avatar`: another avatar's
    /// states than the worn one's (the window's States page: its default, names, options and deletes only).
    States {
        action: StateAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        avatar: Option<String>,
    },
    /// Tracking smoothing as N4N0's menu set it (its Tracking page): on or off, each kind's time constant in
    /// milliseconds (headset, controllers, trackers).
    Tracking {
        on: bool,
        ms: [u16; 3],
    },
    /// A feature suggestion for Orbiters from N4N0's menu (`voice`: some of it was said to N4N0).
    Suggest {
        text: String,
        voice: bool,
    },
    /// N4N0 listens to the next sentence for the menu (Suggest features' Talk).
    Dictate,
    /// Opens a video on the computer (the world's, from the Music page).
    Watch {
        link: String,
    },
    /// A copied link dropped in the world as a card, which the avatar's screen shows (contracts `Docs/n4n0.md`, Link card).
    Drop {
        link: String,
        title: String,
        /// The thumbnail's key (the video id).
        video: Option<String>,
    },
    /// The link card out in the world taken back (as when its time is up): from N4N0's menu, the overlay or the window.
    TakeBack,
    /// Orbiters' VRChat account invites the player to this previous instance (`wrld_…:<instance>`; N4N0's menu, More,
    /// Previous instances): its answer is the status' `invite`.
    InviteMe {
        location: String,
    },
    /// The QR code scanner reads VRChat's stream camera (`on`: its page opened, Scan again) or no longer (its page closed):
    /// what it does is the status' `qr`.
    Qr {
        on: bool,
        /// The menu asking: the scanner runs while any menu holds it, so one menu's stop never ends another's scan.
        #[serde(default)]
        holder: MenuHolder,
    },
    /// The overlay's N4N0 menu opened History: the 3D captures listed again (the status' `captures`).
    Captures,
    /// The overlay's Share to … opened (`on`) or closed: the VRChat friends read again (the status' `friends`), or
    /// forgotten.
    Friends {
        on: bool,
        /// The menu asking: the friends are kept while any menu holds them.
        #[serde(default)]
        holder: MenuHolder,
    },
    /// A 3D capture shared from the overlay's menu: how it goes comes in its row of the status' `captures`.
    ShareCapture {
        share: orbiters_n4n0_core::captures::ShareAsk,
    },
    /// World feedback the overlay's menu asked (read a world's, like, label, comment): what the app knows comes in the
    /// status' `feedback`.
    Feedback {
        ask: orbiters_n4n0_core::feedback::FeedbackAsk,
    },
    /// A controller's trigger pulled or let go (the SteamVR overlay reads it): on a screen of version 7 it presses where
    /// that hand's index finger points.
    Trigger {
        right: bool,
        down: bool,
    },
    /// Each hand's touchpad and thumbstick as the SteamVR overlay read them (contracts `Docs/n4n0-input.md`), paced by
    /// [`crate::input::Pacer`]: the avatar's N4N0 input parameters follow them.
    Input {
        pads: crate::input::Pads,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window and the overlay send these to the service as JSON: their shape stays what it was in the desktop.
    #[test]
    fn commands_keep_their_json() {
        let json = |c: &Command| serde_json::to_string(c).unwrap();
        assert_eq!(json(&Command::Talk { down: true }), r#"{"kind":"talk","down":true}"#);
        assert_eq!(json(&Command::Photo3DIn { seconds: 3 }), r#"{"kind":"photo_3d_in","seconds":3}"#);
        assert_eq!(json(&Command::Photo3D), r#"{"kind":"photo3_d"}"#);
        assert_eq!(json(&Command::TestFace { face: Some(N4n0Face::Happy) }), r#"{"kind":"test_face","face":"happy"}"#);
        assert_eq!(
            json(&Command::States { action: StateAction::Save { name: None, size: true, pose: false, default: false }, avatar: None }),
            r#"{"kind":"states","action":{"do":"save","name":null,"size":true,"pose":false,"default":false}}"#
        );
        let back: Command = serde_json::from_str(r#"{"kind":"drop","link":"https://x.y/","title":"t","video":null}"#).unwrap();
        assert_eq!(back, Command::Drop { link: "https://x.y/".into(), title: "t".into(), video: None });
        // Another avatar's states: the avatar named; the worn one's read as before.
        let other = Command::States { action: StateAction::Delete { id: "s1".into() }, avatar: Some("avtr_b".into()) };
        assert_eq!(json(&other), r#"{"kind":"states","action":{"do":"delete","id":"s1"},"avatar":"avtr_b"}"#);
        let worn: Command = serde_json::from_str(r#"{"kind":"states","action":{"do":"delete","id":"s1"}}"#).unwrap();
        assert_eq!(worn, Command::States { action: StateAction::Delete { id: "s1".into() }, avatar: None });
        assert_eq!(json(&Command::TakeBack), r#"{"kind":"take_back"}"#);
        assert_eq!(json(&Command::InviteMe { location: "wrld_a:1".into() }), r#"{"kind":"invite_me","location":"wrld_a:1"}"#);
        assert_eq!(json(&Command::Qr { on: true, holder: MenuHolder::Overlay }), r#"{"kind":"qr","on":true,"holder":"overlay"}"#);
        assert_eq!(json(&Command::Friends { on: false, holder: MenuHolder::Window }), r#"{"kind":"friends","on":false,"holder":"window"}"#);
        // An older sender's command, without a holder.
        let old: Command = serde_json::from_str(r#"{"kind":"qr","on":true}"#).unwrap();
        assert_eq!(old, Command::Qr { on: true, holder: MenuHolder::Any });
        let share = Command::ShareCapture {
            share: orbiters_n4n0_core::captures::ShareAsk {
                key: "C:/s/a".into(),
                public: false,
                recipients: vec![orbiters_n4n0_core::captures::Friend { id: "usr_a".into(), name: "A".into() }],
                acknowledge_explicit: false,
            },
        };
        let back: Command = serde_json::from_str(&json(&share)).unwrap();
        assert_eq!(back, share);
    }
}
