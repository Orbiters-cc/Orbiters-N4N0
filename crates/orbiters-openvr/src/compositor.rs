//! `IVRCompositor_029`: the scene application's frame timing, as the compositor saw it (layouts from Valve's public
//! `openvr_capi.h`).

use crate::{entry, Client, Error, Pose};

pub const COMPOSITOR_VERSION: &str = "IVRCompositor_029";

/// `Compositor_FrameTiming` (192 bytes): one frame of the scene application.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct FrameTiming {
    pub size: u32,
    pub frame_index: u32,
    pub num_frame_presents: u32,
    pub num_mis_presented: u32,
    pub num_dropped_frames: u32,
    pub reprojection_flags: u32,
    pub system_time_s: f64,
    pub pre_submit_gpu_ms: f32,
    pub post_submit_gpu_ms: f32,
    pub total_render_gpu_ms: f32,
    pub compositor_render_gpu_ms: f32,
    pub compositor_render_cpu_ms: f32,
    pub compositor_idle_cpu_ms: f32,
    /// Time between the scene application's last two frames: its frame rate is `1000 / client_frame_interval_ms`.
    pub client_frame_interval_ms: f32,
    pub present_call_cpu_ms: f32,
    pub wait_for_present_cpu_ms: f32,
    pub submit_frame_ms: f32,
    pub wait_get_poses_called_ms: f32,
    pub new_poses_ready_ms: f32,
    pub new_frame_ready_ms: f32,
    pub compositor_update_start_ms: f32,
    pub compositor_update_end_ms: f32,
    pub compositor_render_start_ms: f32,
    pub hmd_pose: Pose,
    pub num_vsyncs_ready_for_use: u32,
    pub num_vsyncs_to_first_view: u32,
    pub transfer_latency_ms: f32,
}

pub struct Compositor<'a> {
    table: *const usize,
    _client: &'a Client,
}

impl Client {
    pub fn compositor(&self) -> Result<Compositor<'_>, Error> {
        Ok(Compositor { table: self.interface(COMPOSITOR_VERSION)?, _client: self })
    }
}

impl Compositor<'_> {
    /// The last frames (oldest first), at most `out.len()`; how many were filled.
    pub fn frame_timings(&self, out: &mut [FrameTiming]) -> usize {
        type F = unsafe extern "system" fn(*mut FrameTiming, u32) -> u32;
        for t in out.iter_mut() {
            t.size = std::mem::size_of::<FrameTiming>() as u32;
        }
        // SAFETY: IVRCompositor_029 entry 11 is GetFrameTimings; each entry's m_nSize is set.
        let n = unsafe { entry::<F>(self.table, 11)(out.as_mut_ptr(), out.len() as u32) };
        (n as usize).min(out.len())
    }

    /// The process id of the application that has the scene focus (0 for none).
    pub fn scene_focus_process(&self) -> u32 {
        type F = unsafe extern "system" fn() -> u32;
        // SAFETY: entry 24 is GetCurrentSceneFocusProcess.
        unsafe { entry::<F>(self.table, 24)() }
    }

    /// The scene application shows a loading screen.
    pub fn is_scene_loading(&self) -> bool {
        type F = unsafe extern "system" fn() -> bool;
        // SAFETY: entry 47 is IsCurrentSceneFocusAppLoading.
        unsafe { entry::<F>(self.table, 47)() }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frame_timing_matches_openvr() {
        assert_eq!(std::mem::size_of::<super::FrameTiming>(), 192);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, system_time_s), 24);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, client_frame_interval_ms), 56);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, hmd_pose), 96);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, transfer_latency_ms), 184);
    }
}
