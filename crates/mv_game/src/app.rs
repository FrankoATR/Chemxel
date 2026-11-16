//! Application state and the winit event handler.
//!
//! Each frame: measure elapsed time, run as many fixed simulation ticks as the
//! [`FixedTimestep`] allows, then render once. Simulation and rendering are fully
//! decoupled; rendering never mutates simulation state.

use std::sync::Arc;

use anyhow::Context as _;
use mv_core::{Config, FixedTimestep};
use mv_render::{GpuContext, Renderer, SurfaceSize};
use mv_sim::Simulation;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::frame_timer::FrameTimer;

/// Everything that only exists while a window is open.
struct Running {
    window: Arc<Window>,
    renderer: Renderer,
}

pub struct App {
    config: Config,
    sim: Simulation,
    timestep: FixedTimestep,
    frame_timer: FrameTimer,
    running: Option<Running>,
    /// Fatal error captured inside the event loop, reported after it exits.
    fatal: Option<anyhow::Error>,
}

impl App {
    pub fn new(config: Config) -> Self {
        let timestep = FixedTimestep::new(config.sim.tick_rate_hz, config.sim.max_ticks_per_frame);
        Self {
            config,
            sim: Simulation::new(),
            timestep,
            frame_timer: FrameTimer::new(),
            running: None,
            fatal: None,
        }
    }

    /// Consumes the app after the event loop exits, surfacing any fatal error.
    pub fn into_result(self) -> anyhow::Result<()> {
        match self.fatal {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn create_running(&self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
        let wc = &self.config.window;
        let attributes = Window::default_attributes()
            .with_title(wc.title.clone())
            .with_inner_size(LogicalSize::new(wc.width, wc.height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("creating the window")?,
        );

        let size = to_surface_size(window.inner_size());
        let ctx = pollster::block_on(GpuContext::new(
            Arc::clone(&window),
            size,
            self.config.render.vsync,
        ))
        .context("initialising the GPU")?;
        let renderer = Renderer::new(ctx, self.config.render.clear_color);
        Ok(Running { window, renderer })
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        tracing::error!("{error:#}");
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn redraw(&mut self) {
        let (dt, fps) = self.frame_timer.tick();

        let ticks = self.timestep.advance(dt);
        for _ in 0..ticks {
            self.sim.step();
        }

        let Some(running) = self.running.as_mut() else {
            return;
        };
        running.renderer.render();

        if let Some(fps) = fps {
            let title = format!(
                "{} — {fps:.0} FPS ({:.2} ms) — tick {}",
                self.config.window.title,
                self.frame_timer.frame_ms(),
                self.sim.tick()
            );
            running.window.set_title(&title);
            tracing::debug!(
                fps = self.frame_timer.fps(),
                tick = self.sim.tick(),
                dropped = self.timestep.dropped_ticks(),
                "frame stats"
            );
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.create_running(event_loop) {
            Ok(running) => {
                self.running = Some(running);
                self.frame_timer.reset();
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(running) = self.running.as_mut() {
                    running.renderer.resize(to_surface_size(size));
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(running) = &self.running {
            running.window.request_redraw();
        }
    }
}

fn to_surface_size(size: PhysicalSize<u32>) -> SurfaceSize {
    SurfaceSize::new(size.width, size.height)
}
