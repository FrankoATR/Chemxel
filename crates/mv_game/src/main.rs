//! Chemxel entry point.

mod app;
mod fly_camera;
mod frame_timer;
mod input;
mod logging;
mod scene;

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
    let scene = scene::Scene::test_scene(&registry)?;

    let event_loop = EventLoop::new().context("creating the event loop")?;
    // Games redraw continuously; the frame rate is limited by vsync (if enabled).
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(config, registry, scene);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    app.into_result()
}
