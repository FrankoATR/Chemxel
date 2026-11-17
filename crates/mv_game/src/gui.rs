//! egui integration: winit input → egui → wgpu, drawn on top of the frame.

use egui_wgpu::ScreenDescriptor;
use mv_render::{GpuContext, OverlayTarget};
use winit::event::WindowEvent;
use winit::window::Window;

/// Tessellated UI of one frame, ready to be painted.
#[derive(Default)]
pub struct GuiFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    pixels_per_point: f32,
}

pub struct Gui {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// Texture changes not uploaded yet. They accumulate until a frame is actually
    /// painted (the renderer may skip frames, e.g. while the surface is not ready),
    /// so none is lost and egui's "unapplied deltas" check never fires.
    pending_textures: egui::TexturesDelta,
}

impl std::fmt::Debug for Gui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gui").finish_non_exhaustive()
    }
}

impl Gui {
    pub fn new(window: &Window, gpu: &GpuContext) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.overlay_format(),
            egui_wgpu::RendererOptions::default(),
        );
        Self {
            ctx,
            state,
            renderer,
            pending_textures: egui::TexturesDelta::default(),
        }
    }

    /// Feeds a window event to egui. Returns `true` if egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Whether the pointer is over (or dragging) a UI element.
    pub fn wants_pointer(&self) -> bool {
        self.ctx.egui_wants_pointer_input()
    }

    /// Runs one UI pass and tessellates it.
    pub fn run(&mut self, window: &Window, ui: impl FnMut(&egui::Context)) -> GuiFrame {
        let input = self.state.take_egui_input(window);
        let output = self.ctx.run_pass(input, ui);
        self.state
            .handle_platform_output(window, output.platform_output);
        self.pending_textures.append(output.textures_delta);
        GuiFrame {
            primitives: self.ctx.tessellate(output.shapes, output.pixels_per_point),
            pixels_per_point: output.pixels_per_point,
        }
    }

    /// Records the UI draw onto `target`. Returns command buffers that must be
    /// submitted before `encoder`.
    pub fn paint(
        &mut self,
        frame: &GuiFrame,
        target: &OverlayTarget<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Vec<wgpu::CommandBuffer> {
        let textures = std::mem::take(&mut self.pending_textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.renderer
                    .update_texture(target.device, target.queue, *id, delta);
            }
        }
        let screen = ScreenDescriptor {
            size_in_pixels: [target.size.width, target.size.height],
            pixels_per_point: frame.pixels_per_point,
        };
        let commands = self.renderer.update_buffers(
            target.device,
            target.queue,
            encoder,
            &frame.primitives,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &frame.primitives, &screen);
        }
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        let mut textures = textures;
        textures.clear();
        commands
    }
}
