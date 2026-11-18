//! Chemxel entry point.

mod app;
mod bench_drop;
mod bench_fly;
mod drop_test;
mod editor;
mod fly_camera;
mod frame_timer;
mod game_world;
mod gui;
mod hot_reload;
mod input;
mod logging;
mod overlay;

use anyhow::Context as _;
use mv_materials::{MaterialRegistry, ReactionSet};
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
    let reactions =
        ReactionSet::load_dir(&assets.join("reactions"), &registry).context("loading reactions")?;
    // Debug builds reload the RON files when they change; release builds on request.
    let hot_reload = (cfg!(debug_assertions) || std::env::args().any(|a| a == "--hot-reload"))
        .then(|| {
            tracing::info!("hot reload of materials and reactions enabled");
            hot_reload::HotReload::new(vec![assets.join("materials"), assets.join("reactions")])
        });
    let world = game_world::GameWorld::new(&config, &registry)?;
    let secs = std::time::Duration::from_secs_f64;
    let bench_drop = seconds_arg("--bench-drop")
        .map(|s| bench_drop::BenchDrop::new(drop_test::Scenario::SandAndWater, secs(s)))
        .or_else(|| {
            seconds_arg("--bench-heat")
                .map(|s| bench_drop::BenchDrop::new(drop_test::Scenario::LavaOnIce, secs(s)))
        })
        .or_else(|| {
            seconds_arg("--bench-fire")
                .map(|s| bench_drop::BenchDrop::new(drop_test::Scenario::ForestFire, secs(s)))
        })
        .or_else(|| {
            seconds_arg("--bench-tower")
                .map(|s| bench_drop::BenchDrop::new(drop_test::Scenario::TowerCollapse, secs(s)))
        });
    let bench = seconds_arg("--bench-fly").map(|seconds| {
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

    let materials = app::Materials {
        registry,
        reactions,
        assets,
        hot_reload,
    };
    let mut app = app::App::new(config, materials, world, bench, bench_drop);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    app.into_result()
}

/// `--NAME[=SECONDS]` (default 20 s).
fn seconds_arg(name: &str) -> Option<f64> {
    std::env::args().find_map(|arg| {
        let rest = arg.strip_prefix(name)?;
        match rest.strip_prefix('=') {
            Some(secs) => secs.parse().ok(),
            None if rest.is_empty() => Some(20.0),
            None => None,
        }
    })
}
