//! Application state and the winit event handler.
//!
//! Each frame: measure elapsed time, run as many fixed simulation ticks as the
//! [`FixedTimestep`] allows, update the camera, upload changed voxels and render
//! once. Simulation and rendering are fully decoupled; rendering never mutates
//! simulation state.

use std::sync::Arc;

use anyhow::Context as _;
use glam::DVec3;
use mv_core::{Config, FixedTimestep};
use mv_materials::MaterialRegistry;
use mv_render::{Camera, GpuContext, Renderer, SurfaceSize};
use mv_sim::Simulation;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::fly_camera::FlyController;
use crate::frame_timer::FrameTimer;
use crate::input::InputState;
use crate::scene::{self, Scene};

/// Everything that only exists while a window is open.
struct Running {
    window: Arc<Window>,
    renderer: Renderer,
}

pub struct App {
    config: Config,
    registry: MaterialRegistry,
    scene: Scene,
    sim: Simulation,
    timestep: FixedTimestep,
    frame_timer: FrameTimer,
    camera: Camera,
    fly: FlyController,
    input: InputState,
    cursor_grabbed: bool,
    running: Option<Running>,
    /// Fatal error captured inside the event loop, reported after it exits.
    fatal: Option<anyhow::Error>,
}

impl App {
    pub fn new(config: Config, registry: MaterialRegistry, scene: Scene) -> Self {
        let timestep = FixedTimestep::new(config.sim.tick_rate_hz, config.sim.max_ticks_per_frame);
        // Start outside the test chunk, looking at its centre.
        let camera = Camera::new(
            DVec3::new(100.0, 58.0, 100.0),
            std::f32::consts::FRAC_PI_4,
            -0.35,
            config.render.fov_y_deg.to_radians(),
        );
        let fly = FlyController::new(&config.controls);
        Self {
            config,
            registry,
            scene,
            sim: Simulation::new(),
            timestep,
            frame_timer: FrameTimer::new(),
            camera,
            fly,
            input: InputState::default(),
            cursor_grabbed: false,
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

    fn create_running(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
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
        let mut renderer = Renderer::new(
            ctx,
            &self
                .scene
                .volume_desc(self.config.render.brick_pool_capacity),
        );
        renderer.set_materials(&scene::gpu_materials(&self.registry));
        self.scene.upload_dirty(&mut renderer);
        tracing::info!(
            "uploaded test scene: {} bricks in use",
            renderer.volume().bricks_in_use()
        );
        Ok(Running { window, renderer })
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        tracing::error!("{error:#}");
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn set_cursor_grab(&mut self, grab: bool) {
        let Some(running) = &self.running else {
            return;
        };
        let window = &running.window;
        if grab {
            // Locked is not supported everywhere (e.g. Windows); fall back to Confined.
            let result = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = result {
                tracing::warn!("could not grab the cursor: {e}");
                return;
            }
        } else if let Err(e) = window.set_cursor_grab(CursorGrabMode::None) {
            tracing::warn!("could not release the cursor: {e}");
        }
        window.set_cursor_visible(!grab);
        self.cursor_grabbed = grab;
        self.input.take_mouse_delta();
    }

    fn redraw(&mut self) {
        let (dt, fps) = self.frame_timer.tick();

        let ticks = self.timestep.advance(dt);
        for _ in 0..ticks {
            self.sim.step();
        }

        self.fly.update(
            &mut self.camera,
            &mut self.input,
            dt.as_secs_f32(),
            self.cursor_grabbed,
        );

        let Some(running) = self.running.as_mut() else {
            return;
        };
        self.scene.upload_dirty(&mut running.renderer);
        running.renderer.render(&self.camera);

        if let Some(fps) = fps {
            let p = self.camera.position;
            let title = format!(
                "{} — {fps:.0} FPS ({:.2} ms) — tick {} — pos ({:.0}, {:.0}, {:.0})",
                self.config.window.title,
                self.frame_timer.frame_ms(),
                self.sim.tick(),
                p.x,
                p.y,
                p.z
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

    fn on_key(&mut self, event_loop: &ActiveEventLoop, key: KeyCode, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if key == KeyCode::Escape && pressed {
            if self.cursor_grabbed {
                self.set_cursor_grab(false);
            } else {
                event_loop.exit();
            }
            return;
        }
        self.input.set_key(key, pressed);
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
                        physical_key: PhysicalKey::Code(key),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => self.on_key(event_loop, key, state),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !self.cursor_grabbed => self.set_cursor_grab(true),
            WindowEvent::Focused(false) => {
                self.input.release_all();
                self.set_cursor_grab(false);
            }
            WindowEvent::Resized(size) => {
                if let Some(running) = self.running.as_mut() {
                    running.renderer.resize(to_surface_size(size));
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && self.cursor_grabbed
        {
            self.input.add_mouse_motion(dx, dy);
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
