//! Chemxel entry point.

mod app;
mod frame_timer;
mod logging;

use anyhow::Context as _;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    logging::init();

    let assets = mv_core::paths::assets_dir();
    match &assets {
        Some(dir) => tracing::info!("assets directory: {}", dir.display()),
        None => tracing::warn!("assets directory not found"),
    }
    let config = mv_core::Config::load(assets.as_deref()).context("loading configuration")?;

    let event_loop = EventLoop::new().context("creating the event loop")?;
    // Games redraw continuously; the frame rate is limited by vsync (if enabled).
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(config);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    app.into_result()
}
