//! GPU renderer built on wgpu.
//!
//! Voxels are drawn by a compute shader that raymarches a brickmap with a
//! hierarchical DDA (bricks, then voxels), writes an HDR texture, and a blit pass
//! tonemaps it onto the swapchain. See `PROMPT.md` §7.

mod blit;
mod bodies;
mod camera;
mod context;
mod gpu_volume;
mod heat;
mod raymarch;
mod renderer;
mod slots;

pub use bodies::BodyInstance;
pub use camera::Camera;
pub use context::{GpuContext, SurfaceSize};
pub use gpu_volume::{
    BRICK_BYTES, EMPTY, GpuVolume, MAX_MATERIALS, MaterialGpu, UNIFORM_BIT, UploadStats,
    VolumeDesc, VolumeStats,
};
pub use heat::HeatField;
pub use renderer::{BrushPreview, FrameStats, OverlayTarget, Renderer};

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

#[cfg(test)]
mod shader_tests {
    use wgpu::naga;

    /// Parses and validates a WGSL module the way wgpu does at pipeline creation,
    /// so shader errors fail `cargo test` instead of the game at startup.
    fn validate(name: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    }

    #[test]
    fn raymarch_shader_is_valid() {
        validate("raymarch.wgsl", crate::raymarch::SHADER);
    }

    #[test]
    #[should_panic(expected = "broken.wgsl")]
    fn validation_catches_errors() {
        validate("broken.wgsl", "fn main( {");
    }

    #[test]
    fn blit_shader_is_valid() {
        validate("blit.wgsl", crate::blit::SHADER);
    }
}
