//! GPU renderer built on wgpu.
//!
//! Milestone 0 only clears the screen. From milestone 1 on this crate hosts the
//! brickmap upload and the compute-shader voxel raymarcher (see `PROMPT.md` §7).

mod context;
mod renderer;

pub use context::{GpuContext, SurfaceSize};
pub use renderer::{FrameStats, Renderer};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("failed to create the window surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("no suitable GPU adapter found: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to create the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("the surface is not supported by the selected adapter")]
    UnsupportedSurface,
}
