//! Frame orchestration: uploads, raymarch compute pass, blit, overlay, present.

use glam::{DVec3, Vec3};
use mv_core::ChunkPos;
use mv_world::Chunk;

use crate::blit::BlitPass;
use crate::bodies::{BodyInstance, BodyVolume};
use crate::camera::Camera;
use crate::context::{GpuContext, SurfaceSize};
use crate::gpu_volume::{GpuVolume, MaterialGpu, UploadStats, VolumeDesc, VolumeStats};
use crate::heat::{HeatField, HeatVolume};
use crate::raymarch::{FrameUniform, RaymarchPass};

/// Per-frame information returned to the caller (for overlays and logs).
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    /// Whether a frame was actually presented (false when the surface was skipped).
    pub presented: bool,
}

/// Brush sphere highlighted in the frame, in world microvoxel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct BrushPreview {
    pub center: DVec3,
    pub radius: f32,
}

/// What an overlay (e.g. the debug UI) needs to draw on top of the frame.
#[derive(Debug)]
pub struct OverlayTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Swapchain view (format [`GpuContext::overlay_format`], i.e. not sRGB),
    /// already containing the tonemapped frame.
    pub view: &'a wgpu::TextureView,
    pub size: SurfaceSize,
}

/// Top-level renderer. Owns the GPU context, the voxel volume and every pass.
#[derive(Debug)]
pub struct Renderer {
    ctx: GpuContext,
    volume: GpuVolume,
    raymarch: RaymarchPass,
    blit: BlitPass,
    heat: HeatVolume,
    bodies: BodyVolume,
    heat_view: bool,
    ambient: f32,
    sun_dir: Vec3,
    /// Fog: distance where it starts and coefficient k (see `set_view_distance`).
    fog_start: f32,
    fog_k: f32,
}

impl Renderer {
    pub fn new(ctx: GpuContext, volume: &VolumeDesc) -> Self {
        let heat = HeatVolume::new(&ctx.device, volume.dims, volume.heat_capacity);
        let bodies = BodyVolume::new(
            &ctx.device,
            volume.body_capacity,
            volume.body_voxel_capacity,
        );
        let volume = GpuVolume::new(&ctx.device, volume);
        let raymarch = RaymarchPass::new(&ctx.device, &volume, &heat, &bodies, ctx.size());
        let blit = BlitPass::new(&ctx.device, ctx.surface_format(), raymarch.output_view());
        Self {
            ctx,
            volume,
            raymarch,
            blit,
            heat,
            bodies,
            heat_view: false,
            ambient: 20.0,
            sun_dir: Vec3::new(0.45, 0.8, 0.35).normalize(),
            fog_start: 0.0,
            fog_k: 0.0,
        }
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn volume_stats(&self) -> VolumeStats {
        self.volume.stats()
    }

    pub fn resize(&mut self, size: SurfaceSize) {
        if size.is_empty() {
            return;
        }
        self.ctx.resize(size);
        self.raymarch.resize(
            &self.ctx.device,
            &self.volume,
            &self.heat,
            &self.bodies,
            size,
        );
        self.blit
            .set_source(&self.ctx.device, self.raymarch.output_view());
    }

    pub fn set_materials(&mut self, materials: &[MaterialGpu]) {
        self.volume.set_materials(&self.ctx.queue, materials);
    }

    /// Distance in microvoxels at which fog fully hides the scene (the edge of the
    /// loaded world). Fog starts at 35% of it.
    pub fn set_view_distance(&mut self, voxels: f32) {
        // 1 - exp(-((t - start) * k)^2) reaches ~0.97 at t = distance.
        self.fog_start = 0.35 * voxels;
        let span = voxels - self.fog_start;
        self.fog_k = if span > 0.0 { 1.9 / span } else { 0.0 };
    }

    /// Enables the heat-map view. While on, call `upload_heat` each frame.
    pub fn set_heat_view(&mut self, on: bool) {
        self.heat_view = on;
    }

    pub fn heat_view(&self) -> bool {
        self.heat_view
    }

    /// Uploads the temperature fields shown by the heat view; chunks without one
    /// are drawn at `ambient`.
    pub fn upload_heat<'a>(
        &mut self,
        ambient: f32,
        fields: impl Iterator<Item = (ChunkPos, &'a HeatField)>,
    ) {
        self.ambient = ambient;
        self.heat.upload(
            &self.ctx.queue,
            self.volume.origin(),
            self.volume.dims(),
            fields,
        );
    }

    /// Sets the rigid bodies drawn from now on (call every frame they move).
    pub fn set_bodies(&mut self, bodies: &[BodyInstance<'_>]) {
        let origin = self.volume.origin().origin().0.as_dvec3();
        self.bodies.upload(&self.ctx.queue, origin, bodies);
    }

    /// Moves the rendered window of chunks so its minimum corner is `origin`.
    pub fn set_volume_origin(&mut self, origin: ChunkPos) {
        self.volume.set_origin(origin);
    }

    /// Size of the rendered window, in chunks.
    pub fn volume_dims(&self) -> glam::UVec3 {
        self.volume.dims()
    }

    /// Uploads the dirty bricks of a chunk. Cheap if nothing changed.
    pub fn upload_chunk(&mut self, pos: ChunkPos, chunk: &mut Chunk) -> UploadStats {
        self.volume.upload_chunk(&self.ctx.queue, pos, chunk)
    }

    /// Frees the GPU copy of a chunk.
    pub fn remove_chunk(&mut self, pos: ChunkPos) -> UploadStats {
        self.volume.remove_chunk(pos)
    }

    /// Renders and presents one frame from `camera`'s point of view. `overlay` records
    /// extra drawing onto the swapchain view after the voxel image and returns command
    /// buffers that must run before the frame's main encoder.
    pub fn render(
        &mut self,
        camera: &Camera,
        brush: Option<BrushPreview>,
        overlay: impl FnOnce(&OverlayTarget<'_>, &mut wgpu::CommandEncoder) -> Vec<wgpu::CommandBuffer>,
    ) -> FrameStats {
        let _span = tracing::trace_span!("render").entered();
        self.volume.flush(&self.ctx.queue);

        let Some(frame) = self.ctx.acquire_frame() else {
            return FrameStats { presented: false };
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let uniform = self.frame_uniform(camera, brush);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.raymarch
            .dispatch(&self.ctx.queue, &mut encoder, &uniform);
        self.blit.draw(&mut encoder, &target);
        let overlay_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.ctx.overlay_format()),
            ..Default::default()
        });
        let mut before = overlay(
            &OverlayTarget {
                device: &self.ctx.device,
                queue: &self.ctx.queue,
                view: &overlay_view,
                size: self.ctx.size(),
            },
            &mut encoder,
        );
        before.push(encoder.finish());
        self.ctx.queue.submit(before);
        self.ctx.queue.present(frame);
        FrameStats { presented: true }
    }

    fn frame_uniform(&self, camera: &Camera, brush: Option<BrushPreview>) -> FrameUniform {
        let size = self.ctx.size();
        // Subtract in f64 so the shader works with small, precise coordinates.
        let origin = self.volume.origin().origin().0.as_dvec3();
        let rel = (camera.position - origin).as_vec3();
        let dims = self.volume.dims();
        let v4 = |v: Vec3| [v.x, v.y, v.z, 0.0];
        let brush = brush.map_or([0.0; 4], |b| {
            let c = (b.center - origin).as_vec3();
            [c.x, c.y, c.z, b.radius]
        });
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
            brush,
            fog: [self.fog_k, self.fog_start, self.ambient, 0.0],
            volume_dims: [dims.x, dims.y, dims.z, u32::from(self.heat_view)],
            bodies: [self.bodies.count(), 0, 0, 0],
        }
    }
}
