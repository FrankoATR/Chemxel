//! Frame orchestration: uploads, raymarch compute pass, blit, present.

use glam::Vec3;
use mv_core::ChunkPos;
use mv_world::Chunk;

use crate::blit::BlitPass;
use crate::camera::Camera;
use crate::context::{GpuContext, SurfaceSize};
use crate::gpu_volume::{GpuVolume, MaterialGpu, UploadStats, VolumeDesc};
use crate::raymarch::{FrameUniform, RaymarchPass};

/// Per-frame information returned to the caller (for overlays and logs).
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    /// Whether a frame was actually presented (false when the surface was skipped).
    pub presented: bool,
}

/// Top-level renderer. Owns the GPU context, the voxel volume and every pass.
#[derive(Debug)]
pub struct Renderer {
    ctx: GpuContext,
    volume: GpuVolume,
    raymarch: RaymarchPass,
    blit: BlitPass,
    sun_dir: Vec3,
}

impl Renderer {
    pub fn new(ctx: GpuContext, volume: &VolumeDesc) -> Self {
        let volume = GpuVolume::new(&ctx.device, volume);
        let raymarch = RaymarchPass::new(&ctx.device, &volume, ctx.size());
        let blit = BlitPass::new(&ctx.device, ctx.surface_format(), raymarch.output_view());
        Self {
            ctx,
            volume,
            raymarch,
            blit,
            sun_dir: Vec3::new(0.45, 0.8, 0.35).normalize(),
        }
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn volume(&self) -> &GpuVolume {
        &self.volume
    }

    pub fn resize(&mut self, size: SurfaceSize) {
        if size.is_empty() {
            return;
        }
        self.ctx.resize(size);
        self.raymarch.resize(&self.ctx.device, &self.volume, size);
        self.blit
            .set_source(&self.ctx.device, self.raymarch.output_view());
    }

    pub fn set_materials(&mut self, materials: &[MaterialGpu]) {
        self.volume.set_materials(&self.ctx.queue, materials);
    }

    /// Uploads the dirty bricks of a chunk. Cheap if nothing changed.
    pub fn upload_chunk(&mut self, pos: ChunkPos, chunk: &mut Chunk) -> UploadStats {
        self.volume.upload_chunk(&self.ctx.queue, pos, chunk)
    }

    /// Renders and presents one frame from `camera`'s point of view.
    pub fn render(&mut self, camera: &Camera) -> FrameStats {
        self.volume.flush(&self.ctx.queue);

        let Some(frame) = self.ctx.acquire_frame() else {
            return FrameStats { presented: false };
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let uniform = self.frame_uniform(camera);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.raymarch
            .dispatch(&self.ctx.queue, &mut encoder, &uniform);
        self.blit.draw(&mut encoder, &target);
        self.ctx.queue.submit([encoder.finish()]);
        self.ctx.queue.present(frame);
        FrameStats { presented: true }
    }

    fn frame_uniform(&self, camera: &Camera) -> FrameUniform {
        let size = self.ctx.size();
        // Subtract in f64 so the shader works with small, precise coordinates.
        let rel = (camera.position - self.volume.origin().origin().0.as_dvec3()).as_vec3();
        let dims = self.volume.dims();
        let v4 = |v: Vec3| [v.x, v.y, v.z, 0.0];
        FrameUniform {
            camera_pos: v4(rel),
            forward: v4(camera.forward()),
            right: v4(camera.right()),
            up: v4(camera.up()),
            projection: [
                (camera.fov_y * 0.5).tan(),
                size.width as f32 / size.height.max(1) as f32,
                size.width as f32,
                size.height as f32,
            ],
            sun_dir: v4(self.sun_dir),
            volume_dims: [dims.x, dims.y, dims.z, 0],
        }
    }
}
