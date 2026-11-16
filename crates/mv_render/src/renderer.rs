//! Frame orchestration: acquires the swapchain image, records passes, presents.

use crate::context::{GpuContext, SurfaceSize};

/// Per-frame information returned to the caller (for overlays and logs).
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    /// Whether a frame was actually presented (false when the surface was skipped).
    pub presented: bool,
}

/// Top-level renderer. Owns the GPU context and every render pass.
#[derive(Debug)]
pub struct Renderer {
    ctx: GpuContext,
    clear_color: wgpu::Color,
}

impl Renderer {
    pub fn new(ctx: GpuContext, clear_color: [f64; 3]) -> Self {
        let [r, g, b] = clear_color;
        Self {
            ctx,
            clear_color: wgpu::Color { r, g, b, a: 1.0 },
        }
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn resize(&mut self, size: SurfaceSize) {
        self.ctx.resize(size);
    }

    /// Renders and presents one frame.
    pub fn render(&mut self) -> FrameStats {
        let Some(frame) = self.ctx.acquire_frame() else {
            return FrameStats { presented: false };
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        self.ctx.queue.submit([encoder.finish()]);
        self.ctx.queue.present(frame);
        FrameStats { presented: true }
    }
}
