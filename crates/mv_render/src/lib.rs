//! GPU renderer built on wgpu.
//!
//! Voxels are drawn by a compute shader that raymarches a brickmap with a
//! hierarchical DDA (bricks, then voxels), writes an HDR texture, and a blit pass
//! tonemaps it onto the swapchain. See `PROMPT.md` §7.

mod blit;
mod camera;
mod context;
mod gpu_volume;
mod raymarch;
mod renderer;
mod slots;

pub use camera::Camera;
pub use context::{GpuContext, SurfaceSize};
pub use gpu_volume::{EMPTY, GpuVolume, MAX_MATERIALS, MaterialGpu, UploadStats, VolumeDesc};
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
