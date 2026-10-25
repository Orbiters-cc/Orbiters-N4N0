//! What the apps' interfaces ask of N4N0: the window and the SteamVR overlay over the desktop's link, a phone's
//! screen in its own process.

use crate::status::{OrbFace, StateAction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
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
        face: Option<OrbFace>,
    },
    /// Testing: shows every state in turn, two seconds each.
    TestCycle,
    /// Testing: acts as if VRChat said the owner is away (the sleeping orb and the AFK message).
    TestAfk {
        on: bool,
    },
    /// Testing: acts as if the owner touched N4N0.
    TestTouch,
    /// Testing: shows or hides the whole orb (`N4N0/Visible`).
    TestVisible {
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
    /// N4N0's light as N4N0's menu set it (its Light page): `N4N0/Light/On`, its colour and level.
    Light {
        on: bool,
        color: u8,
        level: f32,
    },
    /// The avatar's saved states: save, apply, the default, rename, delete, what one puts back.
    States {
        action: StateAction,
    },
    /// Tracking smoothing as N4N0's menu set it (its Tracking page): on or off, each kind's strength (an index of
    /// `orbiters_orb::menu::SMOOTHING_LABELS`: headset, controllers, trackers).
    Tracking {
        on: bool,
        levels: [u8; 3],
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
    /// A copied link dropped in the world as a card, which the avatar's screen shows (contracts `Docs/orb.md`, Link card).
    Drop {
        link: String,
        title: String,
        /// The thumbnail's key (the video id).
        video: Option<String>,
    },
    /// A controller's trigger pulled or let go (the SteamVR overlay reads it): on a screen of version 7 it presses where
    /// that hand's index finger points.
    Trigger {
        right: bool,
        down: bool,
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
        assert_eq!(json(&Command::TestFace { face: Some(OrbFace::Happy) }), r#"{"kind":"test_face","face":"happy"}"#);
        assert_eq!(
            json(&Command::States { action: StateAction::Save { name: None, size: true, pose: false, default: false } }),
            r#"{"kind":"states","action":{"do":"save","name":null,"size":true,"pose":false,"default":false}}"#
        );
        let back: Command = serde_json::from_str(r#"{"kind":"drop","link":"https://x.y/","title":"t","video":null}"#).unwrap();
        assert_eq!(back, Command::Drop { link: "https://x.y/".into(), title: "t".into(), video: None });
    }
}
