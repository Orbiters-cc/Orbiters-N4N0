//! Synthetic account states: no live network, credentials or session files.
use super::{Page, Scenario};
use crate::controller::vrchat_login::{Picture, Recovery, Status};
use crate::state::AccountState;
use orbiters_core::vrchat_api::{TwoFactor, User};

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in [
        "vrchat-login-signed-out",
        "vrchat-login-entering-password",
        "vrchat-login-totp",
        "vrchat-login-email",
        "vrchat-login-recovery",
        "vrchat-login-signed-in",
        "vrchat-login-error",
        "vrchat-login-busy",
        "vrchat-login-disabled",
        "vrchat-login-orbiters-signed-out",
        "vrchat-login-code-error",
        "vrchat-login-rate-limited",
        "vrchat-login-unavailable",
        "vrchat-login-remove-error",
    ] {
        let mut snapshot = base.clone();
        snapshot.account.state = AccountState::SignedIn { username: "Desktop Tester".into() };
        let mut status = Status { allowed: true, ..Default::default() };
        status.methods = match name {
            "vrchat-login-totp" => vec![TwoFactor::Totp, TwoFactor::Recovery],
            "vrchat-login-email" => vec![TwoFactor::Email],
            "vrchat-login-recovery" => vec![TwoFactor::Recovery, TwoFactor::Totp],
            _ => vec![],
        };
        if name == "vrchat-login-signed-in" {
            status.user = Some(User { id: "usr_screenshot".into(), display_name: "Desktop Tester".into(), ..Default::default() });
            let mut rgba = Vec::new();
            for y in 0..64i32 {
                for x in 0..64i32 {
                    let portrait = (x - 32).pow(2) + (y - 23).pow(2) < 100 || ((x - 32).pow(2) + (y - 66).pow(2) < 625 && y > 36);
                    rgba.extend_from_slice(if portrait { &[172, 218, 207, 255] } else { &[36, 70, 65, 255] });
                }
            }
            status.picture = Some(Picture { width: 64, height: 64, rgba: rgba.into() });
        }
        if name == "vrchat-login-error" {
            status.error = Some("The username or password was not accepted by VRChat.".into());
        }
        if name == "vrchat-login-orbiters-signed-out" {
            snapshot.account.state = AccountState::SignedOut;
        }
        if name == "vrchat-login-code-error" {
            status.methods = vec![TwoFactor::Totp, TwoFactor::Recovery];
            status.error = Some("That code was not accepted. Check it and try again.".into());
        }
        if name == "vrchat-login-rate-limited" {
            status.error = Some("VRChat is rate limiting requests. Try again in 60 seconds.".into());
            status.recovery = Recovery::Resume;
        }
        if name == "vrchat-login-unavailable" {
            status.error = Some("VRChat could not be reached. Try again shortly.".into());
            status.recovery = Recovery::Resume;
        }
        if name == "vrchat-login-remove-error" {
            status.error = Some("The saved VRChat session could not be removed. Try signing out again.".into());
            status.recovery = Recovery::RemoveSavedSession;
        }
        if name == "vrchat-login-busy" {
            status.busy = true;
        }
        if name == "vrchat-login-disabled" {
            status.allowed = false;
        }
        snapshot.vrchat_login = status;
        scenarios.push(Scenario {
            name,
            page: Page::Settings,
            snapshot,
            frames: 100,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
    // Unity projects with a signed-in VRChat account (`pages/projects/vrchat_picture.rs`): the pointer over one of the
    // member's avatar thumbnails, over someone else's, one of them uploading its new picture, and over a world's.
    for name in ["projects-vrchat-picture", "projects-vrchat-picture-theirs", "projects-vrchat-picture-uploading", "projects-vrchat-picture-world"] {
        let mut snapshot = base.clone();
        snapshot.account.state = AccountState::SignedIn { username: "Desktop Tester".into() };
        snapshot.vrchat_login = Status {
            allowed: true,
            user: Some(User { id: "usr_screenshot".into(), display_name: "Desktop Tester".into(), ..Default::default() }),
            upload: (name == "projects-vrchat-picture-uploading").then(|| crate::controller::vrchat_login::PictureUpload {
                serial: 1,
                id: "avtr_00000000-0000-4000-8000-000000000001".into(),
                step: Some(orbiters_core::vrchat_api::Step::Uploading),
                outcome: None,
            }),
            ..Default::default()
        };
        scenarios.push(Scenario {
            name,
            page: Page::Projects,
            snapshot,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
}
