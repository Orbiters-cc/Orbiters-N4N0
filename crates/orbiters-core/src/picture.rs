//! What a 3D picture was taken for, kept with its session (`picture.json`): the reconstruction reads it to know where
//! training's steps go, the gallery to say what the picture is. A session without the file is a free recording, or a
//! picture from before the two modes, which trains as it always did.

use crate::settings::{Photo3dLength, PictureMode};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "picture.json";

/// Whether somebody is out of the picture, as far as the app could tell when it was taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Left {
    /// Nobody asked for it (a picture of people), or there was nobody to leave out.
    #[default]
    NotAsked,
    /// Left out, and VRChat confirmed it.
    Out,
    /// Asked for and not possible: they are in the picture, and the person was told why.
    InPicture,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Picture {
    pub mode: PictureMode,
    pub length: Photo3dLength,
    /// The wearer of the capture kit.
    pub wearer: Left,
    /// Everybody else in the instance.
    pub others: Left,
}

impl Picture {
    pub fn read(session: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(session.join(FILE)).ok()?).ok()
    }

    pub fn save(&self, session: &Path) -> anyhow::Result<()> {
        crate::write_atomic(&session.join(FILE), &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Whether training gives this session's faces steps of their own. A picture of the world has no one to spend them on:
    /// the avatars are out of it, and those a world's walls or posters show are not what it was taken for.
    pub fn trains_faces(session: &Path) -> bool {
        Self::read(session).is_none_or(|p| p.mode != PictureMode::World)
    }
}

/// Someone's choice about a picture's wearer, kept with its session (`wearer.json`): "Reconstruct again" on a picture that
/// shows its wearer writes this, and it wins over everything [`wearer_goes`] would work out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WearerChoice {
    pub take_out: bool,
}

pub const WEARER_FILE: &str = "wearer.json";

impl WearerChoice {
    pub fn read(session: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(session.join(WEARER_FILE)).ok()?).ok()
    }

    pub fn save(&self, session: &Path) -> anyhow::Result<()> {
        crate::write_atomic(&session.join(WEARER_FILE), &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

/// Whether the wearer should be out of this session and their avatar did not do it: reconstruction then takes them out in
/// software (orbiters_reconstruct::wearer).
///
/// In order: the person's own choice; a picture that says they are in it though asked out; a picture that says they are out
/// (hidden, proven by VRChat's echo) has nothing to take out, and one of people where nobody asked keeps them. A recording, or a
/// picture from before the two modes, tells by what VRChat's camera said at the start: Local User off, then the app turned it
/// on (no kit draws without it), and `N4N0/Capture/HideSelf` never came back. Those were Enzo's seven of 2026-11-04.
pub fn wearer_goes(session: &Path) -> bool {
    if let Some(choice) = WearerChoice::read(session) {
        return choice.take_out;
    }
    match Picture::read(session).map(|p| p.wearer) {
        Some(Left::InPicture) => true,
        Some(Left::Out) => false,
        Some(Left::NotAsked) => asked_out_by_the_mask(session),
        None => asked_out_by_the_mask(session),
    }
}

/// What a session with a result can be asked about its wearer, on Reconstruct again's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WearerRow {
    /// Its result shows them, and the skeleton says where they stood: "Reconstruct without my avatar".
    TakeOut,
    /// They were taken out of its result: "Reconstruct with my avatar".
    PutBack,
}

impl WearerRow {
    pub fn label(self) -> &'static str {
        match self {
            WearerRow::TakeOut => tr("Reconstruct without my avatar"),
            WearerRow::PutBack => tr("Reconstruct with my avatar"),
        }
    }

    pub fn takes_out(self) -> bool {
        self == WearerRow::TakeOut
    }
}

/// [`WearerRow`] of a session; `None` without a result, or a skeleton to say where the wearer stood (no kit on the avatar). A
/// picture taken with the wearer really hidden gets the row too: whether a hidden avatar truly vanished is only shown by the
/// picture, and with nothing there the run takes nothing out.
pub fn wearer_row(session: &Path) -> Option<WearerRow> {
    let report = crate::reconstruction::read_report(session)?;
    if report.wearer.is_some() {
        return Some(WearerRow::PutBack);
    }
    session.join(crate::contracts::performance::SKELETON_FILE).is_file().then_some(WearerRow::TakeOut)
}

/// A line for whoever looks at a result, when the wearer was taken out of it or could not be.
pub fn wearer_line(report: &crate::reconstruction::Report) -> Option<&'static str> {
    if report.wearer.is_some() {
        Some(tr("Your avatar was taken out of this 3D view afterwards, as it could not hide itself: what stood behind it comes from other frames. Its shadow may remain."))
    } else if report.wearer_not_found {
        Some(tr("Your avatar should have left this 3D view, and nothing of it was found to take out."))
    } else {
        None
    }
}

/// A line for whoever looks at a result whose world gave no depth for most of what the camera saw ([`Report::depth_blind`]).
pub fn depth_line(report: &crate::reconstruction::Report) -> Option<&'static str> {
    report.depth_blind.then(|| tr("This world gave no depth for most of what the camera saw, so this 3D view was rebuilt from colour alone and looks softer away from the camera's path."))
}

/// Whether the recording began with Local User off in VRChat's camera, and no avatar answered the message that hides it.
fn asked_out_by_the_mask(session: &Path) -> bool {
    use crate::contracts::performance::{self, parameters};
    let Some(table) = std::fs::read_to_string(session.join(performance::PARAMETERS_TABLE)).ok().and_then(|t| serde_json::from_str::<parameters::Table>(&t).ok()) else { return false };
    let Some(changes) = std::fs::read(session.join(performance::PARAMETERS_FILE)).ok().and_then(|b| parameters::read(&b).ok()) else { return false };
    let id_of = |name: &str| table.parameters.iter().find(|p| p.name == name).map(|p| p.id);
    let Some(local) = id_of("/usercamera/LocalPlayer") else { return false };
    let started_off = changes.iter().find(|c| c.id == local).is_some_and(|c| c.value.as_f32() < 0.5);
    let hidden = id_of(crate::osc::HIDE_SELF_PARAMETER).is_some_and(|id| changes.iter().any(|c| c.id == id));
    started_off && !hidden
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The person's word, then the picture's, then what VRChat's camera said when a recording began.
    #[test]
    fn who_goes_is_read_from_the_session() {
        use crate::contracts::performance::{self, parameters};
        let dir = std::env::temp_dir().join(format!("orbiters-wearer-goes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!wearer_goes(&dir), "a session that says nothing keeps them");
        // A recording: Local User off at the start, then on; no echo of the hiding parameter. Enzo's seven of 2026-11-04.
        let write = |local_first: f32, hide: bool| {
            let mut table = parameters::Table::new();
            table.parameters.push(parameters::ParameterInfo { id: 0, name: "/usercamera/LocalPlayer".into(), address: "/usercamera/LocalPlayer".into(), kind: parameters::Kind::Bool });
            if hide {
                table.parameters.push(parameters::ParameterInfo { id: 1, name: crate::osc::HIDE_SELF_PARAMETER.into(), address: "x".into(), kind: parameters::Kind::Bool });
            }
            std::fs::write(dir.join(performance::PARAMETERS_TABLE), serde_json::to_string(&table).unwrap()).unwrap();
            let mut bytes = parameters::header().to_vec();
            bytes.extend(parameters::Change { time_ns: 0, id: 0, flags: parameters::flags::INITIAL, value: parameters::Value::Bool(local_first > 0.5) }.to_bytes());
            bytes.extend(parameters::Change { time_ns: 5, id: 0, flags: 0, value: parameters::Value::Bool(true) }.to_bytes());
            if hide {
                bytes.extend(parameters::Change { time_ns: 6, id: 1, flags: 0, value: parameters::Value::Bool(true) }.to_bytes());
            }
            std::fs::write(dir.join(performance::PARAMETERS_FILE), bytes).unwrap();
        };
        write(0.0, false);
        assert!(wearer_goes(&dir));
        write(1.0, false);
        assert!(!wearer_goes(&dir), "Local User on: they are in the picture, as asked");
        write(0.0, true);
        assert!(!wearer_goes(&dir), "the avatar hid itself");
        write(0.0, false);
        // A picture's word comes before the mask's.
        let world = Picture { mode: PictureMode::World, length: Photo3dLength::Full, wearer: Left::Out, others: Left::NotAsked };
        world.save(&dir).unwrap();
        assert!(!wearer_goes(&dir), "hidden and proven");
        Picture { wearer: Left::InPicture, ..world }.save(&dir).unwrap();
        assert!(wearer_goes(&dir));
        // The person's word comes first of all.
        WearerChoice { take_out: false }.save(&dir).unwrap();
        assert!(!wearer_goes(&dir));
        WearerChoice { take_out: true }.save(&dir).unwrap();
        Picture { wearer: Left::Out, ..world }.save(&dir).unwrap();
        assert!(wearer_goes(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Reconstruct again's side row: none before a result or without a skeleton; out, then back in; the gallery's line says so.
    #[test]
    fn a_result_offers_to_take_its_wearer_out_or_put_them_back() {
        use crate::reconstruction::{self as rec, Report};
        let dir = std::env::temp_dir().join(format!("orbiters-wearer-row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(rec::dir(&dir)).unwrap();
        assert_eq!(wearer_row(&dir), None, "nothing to redo yet");
        let write = |r: &Report| std::fs::write(rec::dir(&dir).join(rec::REPORT_FILE), serde_json::to_string(r).unwrap()).unwrap();
        write(&Report::default());
        assert_eq!(wearer_row(&dir), None, "no kit: nothing says where they stood");
        std::fs::write(dir.join(crate::contracts::performance::SKELETON_FILE), b"x").unwrap();
        assert_eq!(wearer_row(&dir), Some(WearerRow::TakeOut));
        assert_eq!(wearer_line(&rec::read_report(&dir).unwrap()), None);
        let out = Report { wearer: Some(rec::WearerOut { views: 3, ..Default::default() }), ..Default::default() };
        write(&out);
        assert_eq!(wearer_row(&dir), Some(WearerRow::PutBack));
        assert!(!WearerRow::PutBack.takes_out() && WearerRow::TakeOut.takes_out());
        assert!(wearer_line(&out).is_some_and(|l| l.contains("taken out") && l.contains("shadow")));
        assert!(wearer_line(&Report { wearer_not_found: true, ..Default::default() }).is_some_and(|l| l.contains("nothing of it was found")));
        assert_eq!(depth_line(&Report::default()), None);
        assert!(depth_line(&Report { depth_blind: true, ..Default::default() }).is_some_and(|l| l.contains("no depth")));
        // A report from before any of this still reads.
        std::fs::write(rec::dir(&dir).join(rec::REPORT_FILE), r#"{"gaussians":4}"#).unwrap();
        assert_eq!(rec::read_report(&dir).map(|r| r.wearer), Some(None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_world_picture_spends_no_steps_on_faces_and_everything_else_does() {
        let dir = std::env::temp_dir().join(format!("orbiters-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Picture::trains_faces(&dir), "a recording, or a picture from before the two modes");
        let world = Picture { mode: PictureMode::World, length: Photo3dLength::Full, wearer: Left::Out, others: Left::InPicture };
        world.save(&dir).unwrap();
        assert_eq!(Picture::read(&dir), Some(world));
        assert!(!Picture::trains_faces(&dir));
        Picture { mode: PictureMode::People, ..world }.save(&dir).unwrap();
        assert!(Picture::trains_faces(&dir));
        // A file nobody can read says nothing: the session trains as a recording does.
        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert!(Picture::trains_faces(&dir) && Picture::read(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
