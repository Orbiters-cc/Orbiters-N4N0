//! Reconstruction for Orbiters desktop (implementation plan, phase 3: replay mode). A session's lossless keyframes,
//! exact poses and exact depth samples train a set of 3D Gaussians on the GPU; the result is a splat PLY.

pub mod camera;
pub mod cpu;
pub mod dataset;
pub mod gaussians;
pub mod gpu;
pub mod init;
pub mod ply;
pub mod train;
