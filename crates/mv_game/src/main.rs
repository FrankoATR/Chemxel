//! Chemxel entry point.

mod app;
mod bench_fly;
mod editor;
mod fly_camera;
mod frame_timer;
mod game_world;
mod gui;
mod input;
mod logging;
mod overlay;

use anyhow::Context as _;
use mv_materials::MaterialRegistry;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    logging::init();

    let assets = mv_core::paths::assets_dir().context(
        "assets directory not found (run from the repository root or set CHEMXEL_ASSETS_DIR)",
    )?;
    tracing::info!("assets directory: {}", assets.display());
    let config = mv_core::Config::load(Some(&assets)).context("loading configuration")?;
    let registry =
        MaterialRegistry::load_dir(&assets.join("materials")).context("loading materials")?;
    let world = game_world::GameWorld::new(&config, &registry)?;
    let bench = bench_fly_arg().map(|seconds| {
        let c = &config.controls;
        let speed = c.fly_speed * c.fast_multiplier * mv_core::consts::VOXELS_PER_BLOCK as f32;
        bench_fly::BenchFly::new(
            f64::from(speed),
            std::time::Duration::from_secs_f64(seconds),
        )
    });

    let event_loop = EventLoop::new().context("creating the event loop")?;
    // Games redraw continuously; the frame rate is limited by vsync (if enabled).
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(config, registry, world, bench);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    app.into_result()
}

/// `--bench-fly[=SECONDS]` (default 20 s).
fn bench_fly_arg() -> Option<f64> {
    std::env::args().find_map(|arg| {
        let rest = arg.strip_prefix("--bench-fly")?;
        match rest.strip_prefix('=') {
            Some(secs) => secs.parse().ok(),
            None if rest.is_empty() => Some(20.0),
            None => None,
        }
    })
}
