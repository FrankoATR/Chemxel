//! Application state and the winit event handler.
//!
//! Each frame: measure elapsed time, run as many fixed simulation ticks as the
//! [`FixedTimestep`] allows, move the camera, apply brush edits, stream the world
//! and upload changes, build the debug UI and render once. Rendering never mutates
//! simulation state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use mv_core::{Config, FixedTimestep};
use mv_materials::MaterialRegistry;
use mv_render::{Camera, GpuContext, Renderer, SurfaceSize, VolumeStats};
use mv_sim::Simulation;
use mv_world::WorldStats;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::bench_drop::BenchDrop;
use crate::bench_fly::BenchFly;
use crate::drop_test;
use crate::editor::{BrushAction, Editor};
use crate::fly_camera::FlyController;
use crate::frame_timer::FrameTimer;
use crate::game_world::{self, GameWorld, SyncStats};
use crate::gui::Gui;
use crate::input::InputState;
use crate::overlay::{self, OverlayData, Timings};

/// How often the (whole-world) memory statistics are recomputed.
const WORLD_STATS_PERIOD: Duration = Duration::from_millis(500);

/// Everything that only exists while a window is open.
struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    gui: Gui,
}

/// Slow-changing statistics shown by the overlay.
#[derive(Debug, Default)]
struct Stats {
    timings: Timings,
    sync: SyncStats,
    world: WorldStats,
    gpu: VolumeStats,
    world_refreshed: Option<Instant>,
}

pub struct App {
    config: Config,
    registry: MaterialRegistry,
    world: GameWorld,
    sim: Simulation,
    timestep: FixedTimestep,
    frame_timer: FrameTimer,
    camera: Camera,
    fly: FlyController,
    editor: Editor,
    input: InputState,
    cursor_grabbed: bool,
    show_overlay: bool,
    stats: Stats,
    bench: Option<BenchFly>,
    bench_drop: Option<BenchDrop>,
    exit_requested: bool,
    running: Option<Running>,
    /// Fatal error captured inside the event loop, reported after it exits.
    fatal: Option<anyhow::Error>,
}

impl App {
    pub fn new(
        config: Config,
        registry: MaterialRegistry,
        world: GameWorld,
        bench: Option<BenchFly>,
        bench_drop: Option<BenchDrop>,
    ) -> Self {
        let timestep = FixedTimestep::new(config.sim.tick_rate_hz, config.sim.max_ticks_per_frame);
        let camera = Camera::new(
            world.spawn_point(0, 0),
            std::f32::consts::FRAC_PI_4,
            -0.3,
            config.render.fov_y_deg.to_radians(),
        );
        let fly = FlyController::new(&config.controls);
        let editor = Editor::new(&config.editor, &registry);
        let sim = Simulation::new(&registry, &config.sim);
        Self {
            config,
            registry,
            world,
            sim,
            timestep,
            frame_timer: FrameTimer::new(),
            camera,
            fly,
            editor,
            input: InputState::default(),
            cursor_grabbed: false,
            show_overlay: true,
            stats: Stats::default(),
            bench,
            bench_drop,
            exit_requested: false,
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
        let gui = Gui::new(&window, &ctx);
        let mut renderer = Renderer::new(ctx, &GameWorld::volume_desc(&self.config));
        renderer.set_materials(&game_world::gpu_materials(&self.registry));
        let voxels_per_chunk = mv_core::consts::CHUNK_SIZE as f32;
        renderer
            .set_view_distance(self.config.render.render_distance_chunks as f32 * voxels_per_chunk);
        Ok(Running {
            window,
            renderer,
            gui,
        })
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
        } else {
            if let Err(e) = window.set_cursor_grab(CursorGrabMode::None) {
                tracing::warn!("could not release the cursor: {e}");
            }
            self.editor.set_held(None);
        }
        window.set_cursor_visible(!grab);
        self.cursor_grabbed = grab;
        self.input.take_mouse_delta();
    }

    fn redraw(&mut self) {
        let (dt, fps) = self.frame_timer.tick();
        let timings = &mut self.stats.timings;
        timings.frame.add(dt.as_secs_f64() * 1000.0);

        let t = Instant::now();
        let ticks = self.timestep.advance(dt);
        for _ in 0..ticks {
            let tick_start = Instant::now();
            self.sim.step(&mut self.world.world);
            if let Some(b) = self.bench_drop.as_mut() {
                b.record_tick(tick_start.elapsed(), &self.sim.stats());
            }
        }
        if let Some(b) = self.bench_drop.as_mut() {
            if b.should_drop(self.world.is_idle()) {
                let ground = mv_core::WorldPos(
                    self.camera.position.floor().as_i64vec3() - glam::I64Vec3::Y * 48,
                );
                let boxes =
                    drop_test::drop_sand_and_water(&mut self.world.world, &self.registry, ground);
                for (min, max) in boxes {
                    self.sim.wake_box(&self.world.world, min, max);
                }
            } else if b.advance(dt) {
                tracing::info!("{}", b.report());
                self.bench_drop = None;
                self.exit_requested = true;
            }
        }
        if ticks > 0 {
            timings
                .sim
                .add(t.elapsed().as_secs_f64() * 1000.0 / f64::from(ticks));
        }

        if let Some(bench) = self.bench.as_mut() {
            let ready = self.world.is_idle();
            if bench.step(&mut self.camera, dt, ready, &self.stats.sync) {
                let w = self.world.world.stats();
                let g = &self.stats.gpu;
                tracing::info!(
                    "{}; world: {} chunks ({} uniform), {} dense bricks, {:.0} MiB CPU; GPU: {:.0} MiB bricks, {} chunk slots",
                    bench.report(),
                    w.chunks,
                    w.uniform_chunks,
                    w.dense_bricks,
                    w.heap_bytes as f64 / (1024.0 * 1024.0),
                    g.brick_bytes() as f64 / (1024.0 * 1024.0),
                    g.chunk_slots_in_use
                );
                self.bench = None;
                self.exit_requested = true;
            }
        } else {
            self.fly.update(
                &mut self.camera,
                &mut self.input,
                dt.as_secs_f32(),
                self.cursor_grabbed,
            );
        }

        let t = Instant::now();
        if let Some((min, max)) = self.editor.update(&mut self.world.world, &self.camera, dt) {
            self.sim.wake_box(&self.world.world, min, max);
        }
        timings.editor.add(t.elapsed().as_secs_f64() * 1000.0);

        let Some(running) = self.running.as_mut() else {
            return;
        };

        let t = Instant::now();
        self.stats.sync = self
            .world
            .update(self.camera.position, &mut running.renderer);
        self.stats
            .timings
            .world
            .add(t.elapsed().as_secs_f64() * 1000.0);
        self.stats.gpu = running.renderer.volume_stats();
        let now = Instant::now();
        if self
            .stats
            .world_refreshed
            .is_none_or(|t| now - t >= WORLD_STATS_PERIOD)
        {
            self.stats.world = self.world.world.stats();
            self.stats.world_refreshed = Some(now);
        }

        let sim_stats = self.sim.stats();
        let mut actions = overlay::OverlayActions::default();
        let gui_frame = {
            let data = OverlayData {
                fps: self.frame_timer.fps(),
                worst_frame_ms: self.frame_timer.worst_frame_ms(),
                timings: &self.stats.timings,
                tick: self.sim.tick(),
                sim: &sim_stats,
                sim_active: self.sim.active_chunks(),
                sim_paused: self.sim.is_paused(),
                camera: self.camera.position,
                sync: &self.stats.sync,
                world: &self.stats.world,
                gpu: &self.stats.gpu,
                registry: &self.registry,
                cursor_grabbed: self.cursor_grabbed,
            };
            let editor = &mut self.editor;
            let (show_overlay, grabbed) = (self.show_overlay, self.cursor_grabbed);
            running.gui.run(&running.window, |ctx| {
                if show_overlay {
                    actions = overlay::draw(ctx, &data, editor);
                }
                if grabbed {
                    overlay::crosshair(ctx);
                }
            })
        };

        self.apply_overlay_actions(actions);
        let Some(running) = self.running.as_mut() else {
            return;
        };

        let t = Instant::now();
        let gui = &mut running.gui;
        running
            .renderer
            .render(&self.camera, self.editor.preview(), |target, encoder| {
                gui.paint(&gui_frame, target, encoder)
            });
        self.stats
            .timings
            .render
            .add(t.elapsed().as_secs_f64() * 1000.0);

        if let Some(fps) = fps {
            let p = self.camera.position;
            let title = format!(
                "{} — {fps:.0} FPS ({:.2} ms, worst {:.1} ms) — {} chunks — pos ({:.0}, {:.0}, {:.0})",
                self.config.window.title,
                self.frame_timer.frame_ms(),
                self.frame_timer.worst_frame_ms(),
                self.world.world.len(),
                p.x,
                p.y,
                p.z
            );
            running.window.set_title(&title);
            tracing::debug!(
                fps = self.frame_timer.fps(),
                tick = self.sim.tick(),
                dropped = self.timestep.dropped_ticks(),
                chunks = self.world.world.len(),
                bricks = self.stats.gpu.bricks_in_use,
                "frame stats"
            );
        }
    }

    fn apply_overlay_actions(&mut self, actions: overlay::OverlayActions) {
        if actions.toggle_pause {
            self.sim.set_paused(!self.sim.is_paused());
        }
        if actions.drop_test {
            // Above the voxel under the crosshair, or below the camera.
            let ground = self.editor.target().map_or_else(
                || {
                    mv_core::WorldPos(
                        self.camera.position.floor().as_i64vec3() - glam::I64Vec3::Y * 60,
                    )
                },
                |hit| hit.voxel,
            );
            let boxes =
                drop_test::drop_sand_and_water(&mut self.world.world, &self.registry, ground);
            for (min, max) in boxes {
                self.sim.wake_box(&self.world.world, min, max);
            }
        }
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, key: KeyCode, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if pressed {
            match key {
                KeyCode::Escape => {
                    if self.cursor_grabbed {
                        self.set_cursor_grab(false);
                    } else {
                        event_loop.exit();
                    }
                    return;
                }
                KeyCode::F3 => self.show_overlay = !self.show_overlay,
                KeyCode::KeyP => self.sim.set_paused(!self.sim.is_paused()),
                _ => {
                    if let Some(index) = digit_index(key) {
                        self.editor.select(index);
                    }
                }
            }
        }
        self.input.set_key(key, pressed);
    }

    fn on_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if !self.cursor_grabbed {
            if pressed && button == MouseButton::Left {
                self.set_cursor_grab(true);
            }
            return;
        }
        let action = match button {
            MouseButton::Left => BrushAction::Remove,
            MouseButton::Right => BrushAction::Place,
            _ => return,
        };
        if pressed {
            self.editor.set_held(Some(action));
        } else {
            self.editor.set_held(None);
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
        // The UI gets every event first; while the mouse is free it may claim them.
        let ui_consumed = match self.running.as_mut() {
            Some(r) => {
                let consumed = r.gui.on_window_event(&r.window, &event);
                !self.cursor_grabbed && (consumed || r.gui.wants_pointer())
            }
            None => false,
        };

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
            } => {
                // Releases always go through so no key stays stuck.
                if !ui_consumed || state == ElementState::Released {
                    self.on_key(event_loop, key, state);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if !ui_consumed || state == ElementState::Released {
                    self.on_mouse_button(button, state);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.cursor_grabbed => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.editor.adjust_radius(steps);
            }
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit_requested {
            event_loop.exit();
            return;
        }
        if let Some(running) = &self.running {
            running.window.request_redraw();
        }
    }
}

/// `1`..`9` → `0`..`8`.
fn digit_index(key: KeyCode) -> Option<usize> {
    const DIGITS: [KeyCode; 9] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    DIGITS.iter().position(|&k| k == key)
}

fn to_surface_size(size: PhysicalSize<u32>) -> SurfaceSize {
    SurfaceSize::new(size.width, size.height)
}
