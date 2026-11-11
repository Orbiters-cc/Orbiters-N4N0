//! `IVRCompositor_029`: the scene application's frame timing, as the compositor saw it (layouts from Valve's public
//! `openvr_capi.h`).

use crate::{entry, Client, Error, Pose};
use std::ffi::c_void;

pub const COMPOSITOR_VERSION: &str = "IVRCompositor_029";

/// The left or right eye's picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Left = 0,
    Right = 1,
}

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
        Ok(Compositor { table: self.table(&self.compositor_table, COMPOSITOR_VERSION)?, _client: self })
    }
}

/// What reads an eye's picture from another thread than the client's. Whoever makes one keeps the client alive until it is dropped.
#[derive(Clone, Copy, Debug)]
pub struct MirrorSource {
    table: *const usize,
}

// SAFETY: only the table's address, of a process-wide interface; GetMirrorTextureD3D11 is called from other threads by every mirror reader.
unsafe impl Send for MirrorSource {}

impl MirrorSource {
    /// Latest compositor frame, for rejecting eye reads that straddle a frame boundary.
    pub fn frame_index(&self) -> Option<u32> {
        type F = unsafe extern "system" fn(*mut FrameTiming, u32) -> bool;
        let mut timing=FrameTiming { size:std::mem::size_of::<FrameTiming>() as u32,..Default::default() };
        // SAFETY: IVRCompositor_029 entry 10 is GetFrameTiming. The caller retains the live Client.
        unsafe { entry::<F>(self.table,10)(&mut timing,0) }.then_some(timing.frame_index)
    }
    /// See [`Compositor::mirror_d3d11`].
    ///
    /// # Safety
    /// `device` is a live `ID3D11Device*` on the headset's GPU, and the client this came from is alive.
    pub unsafe fn mirror_d3d11(&self, eye: Eye, device: *mut c_void) -> Result<*mut c_void, i32> {
        type F = unsafe extern "system" fn(i32, *mut c_void, *mut *mut c_void) -> i32;
        let mut view = std::ptr::null_mut();
        // SAFETY: IVRCompositor_029 entry 35 is GetMirrorTextureD3D11 (two entries past _027's, as every entry from
        // GetFrameTimings on); the out parameter is valid.
        let error = unsafe { entry::<F>(self.table, 35)(eye as i32, device, &mut view) };
        if error == 0 && !view.is_null() {
            Ok(view)
        } else {
            Err(error)
        }
    }

    /// # Safety
    /// `view` came from [`MirrorSource::mirror_d3d11`] and is not used after.
    pub unsafe fn release_mirror_d3d11(&self, view: *mut c_void) {
        type F = unsafe extern "system" fn(*mut c_void);
        // SAFETY: entry 36 is ReleaseMirrorTextureD3D11, given a view GetMirrorTextureD3D11 returned.
        unsafe { entry::<F>(self.table, 36)(view) }
    }
}

impl Compositor<'_> {
    /// For reading eyes' pictures on a thread of their own.
    pub fn mirror_source(&self) -> MirrorSource {
        MirrorSource { table: self.table }
    }

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
    fn mirror_frame_index_uses_the_sized_timing_record() {
        unsafe extern "system" fn timing(out:*mut super::FrameTiming, ago:u32)->bool {
            assert_eq!(ago,0);
            // SAFETY: frame_index supplies a valid initialized timing record.
            unsafe { assert_eq!((*out).size,std::mem::size_of::<super::FrameTiming>() as u32); (*out).frame_index=77; }
            true
        }
        let mut table=[0usize;11]; table[10]=timing as *const () as usize;
        let source=super::MirrorSource { table:table.as_ptr() };
        assert_eq!(source.frame_index(),Some(77));
    }
    #[test]
    fn frame_timing_matches_openvr() {
        assert_eq!(std::mem::size_of::<super::FrameTiming>(), 192);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, system_time_s), 24);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, client_frame_interval_ms), 56);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, hmd_pose), 96);
        assert_eq!(std::mem::offset_of!(super::FrameTiming, transfer_latency_ms), 184);
    }
}
