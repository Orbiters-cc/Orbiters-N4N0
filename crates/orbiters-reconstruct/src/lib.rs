//! Reconstruction for Orbiters - N4N0 (implementation plan, phase 3: replay mode). A session's lossless keyframes,
//! exact poses and exact depth samples train a set of 3D Gaussians on the GPU; the result is a splat PLY.

pub mod aberration;
pub mod births;
pub mod camera;
pub mod cleanup;
pub mod cpu;
pub mod dataset;
pub mod depthfix;
pub mod dof;
pub mod edit;
pub mod export;
pub mod gaussians;
pub mod governor;
pub mod gpu;
pub mod ground;
pub mod init;
pub mod metrics;
pub mod ply;
pub mod prepare;
pub mod priority;
pub mod select;
pub mod sky;
pub mod timing;
pub mod train;
pub mod tree;
pub mod tree_file;
pub mod tree_tune;
pub mod web;
pub mod mesh;
