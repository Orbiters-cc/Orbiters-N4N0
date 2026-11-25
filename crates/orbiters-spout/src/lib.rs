//! Spout2 interoperability without the C++ SDK: the sender list and sender info live in named shared
//! memory, frames in a shared Direct3D 11 texture guarded by a named mutex (or a keyed mutex), and new
//! frames are signalled through a counting semaphore. Names and layouts follow the Spout2 SDK sources
//! (BSD-2-Clause, Lynn Jarvis / leadedge): `SpoutSenderNames`, `<name>`, `<name>_SpoutAccessMutex`,
//! `<name>_Count_Semaphore`.
//!
//! [`Receiver`] reads any Spout sender (VRChat's Stream camera, OBS, …) into CPU memory, only the rows asked for
//! ([`FrameReader`]); [`Sender`] publishes frames, used by tests and the test-pattern source.
//!
//! On a Mac the same [`Receiver`] reads VRChat's stream camera from Obelisk's engine, which publishes what the game it runs
//! shares for Spout ([`mac`]'s own words): nothing of Spout's is there but what that game says.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(any(windows, target_os = "macos"))]
mod pixels;
#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
pub use mac::{active_sender, list_senders, sender_info, session, set_session, FrameReader, Receiver, Session};
#[cfg(windows)]
pub use win::{active_sender, list_senders, sender_info, FrameReader, Receiver, Sender};

/// Description of a Spout sender, from its shared info block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// DXGI format of the shared texture.
    pub format: u32,
    pub share_handle: u32,
    /// Usually the sender's executable path.
    pub description: String,
}

/// Channel order of received pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelOrder {
    Rgba,
    Bgra,
}

/// A received frame in CPU memory: 4 bytes per pixel, top-down, rows `stride` bytes apart, 8-bit sRGB codes.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: &'a [u8],
    pub order: ChannelOrder,
    /// Sender frame counter when available (increases by one per sent frame).
    pub frame_count: Option<i64>,
    /// When the receiver saw the frame: just before it copied it from the sender's texture.
    pub seen: std::time::Instant,
}

#[derive(Debug)]
pub enum SpoutError {
    NotFound(String),
    UnsupportedFormat(u32),
    Platform(String),
}

impl std::fmt::Display for SpoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpoutError::NotFound(name) => write!(f, "{}", orbiters_i18n::tr!("Spout sender \"{name}\" not found", name = name)),
            SpoutError::UnsupportedFormat(fmt) => write!(f, "{}", orbiters_i18n::tr!("unsupported Spout texture format (DXGI {format})", format = fmt)),
            SpoutError::Platform(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SpoutError {}

pub type Result<T> = std::result::Result<T, SpoutError>;

/// DXGI formats the receiver accepts, and how to read them.
pub mod formats {
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R8G8B8A8_TYPELESS: u32 = 27;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8X8_UNORM: u32 = 88;
    pub const B8G8R8A8_TYPELESS: u32 = 90;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const B8G8R8X8_TYPELESS: u32 = 92;
    pub const B8G8R8X8_UNORM_SRGB: u32 = 93;

    /// Channel order of a directly readable 8-bit format, or `None` when the frame needs conversion.
    pub fn eight_bit_order(format: u32) -> Option<super::ChannelOrder> {
        match format {
            R8G8B8A8_TYPELESS | R8G8B8A8_UNORM | R8G8B8A8_UNORM_SRGB => Some(super::ChannelOrder::Rgba),
            B8G8R8A8_UNORM | B8G8R8X8_UNORM | B8G8R8A8_TYPELESS | B8G8R8A8_UNORM_SRGB | B8G8R8X8_TYPELESS | B8G8R8X8_UNORM_SRGB => {
                Some(super::ChannelOrder::Bgra)
            }
            _ => None,
        }
    }
}
