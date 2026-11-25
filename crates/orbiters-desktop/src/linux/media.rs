//! What plays music on Linux. Nothing yet (the port's sound stream, L3: MPRIS over the session's D-Bus).

/// No media controls to read.
pub struct Media;

impl Media {
    pub fn new() -> orbiters_n4n0::media::NoMedia {
        orbiters_n4n0::media::NoMedia
    }
}
