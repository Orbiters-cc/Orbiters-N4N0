//! What a 3D picture was taken for, kept with its session (`picture.json`): the reconstruction reads it to know where
//! training's steps go, the gallery to say what the picture is. A session without the file is a free recording, or a
//! picture from before the two modes, which trains as it always did.

use crate::settings::{Photo3dLength, PictureMode};
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

#[cfg(test)]
mod tests {
    use super::*;

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
