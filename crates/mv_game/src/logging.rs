//! Logging / tracing setup.
//!
//! Filter with `RUST_LOG` (e.g. `RUST_LOG=debug` or `RUST_LOG=mv_sim=trace`).
//! Records emitted through the `log` crate (wgpu, winit) are forwarded to tracing.

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true));

    #[cfg(feature = "profile")]
    let registry = registry.with(tracing_tracy::TracyLayer::default());

    if let Err(e) = registry.try_init() {
        eprintln!("failed to initialise logging: {e}");
    }
}
