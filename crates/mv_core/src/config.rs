//! Runtime configuration loaded from `assets/config.ron`.
//!
//! Every tuning constant (tick rate, render distance, budgets...) lives here rather
//! than being hard-coded. All fields have defaults, so a partial or missing file is
//! valid.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// File name of the configuration inside the assets directory.
pub const CONFIG_FILE: &str = "config.ron";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse config file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },
    #[error("invalid config value `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
}

/// Root configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub window: WindowConfig,
    pub sim: SimConfig,
    pub render: RenderConfig,
    pub controls: ControlsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Chemxel".to_owned(),
            width: 1600,
            height: 900,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimConfig {
    /// Fixed simulation ticks per second.
    pub tick_rate_hz: f64,
    /// Max ticks run in a single frame before the backlog is dropped.
    pub max_ticks_per_frame: u32,
    /// Seed for deterministic simulation randomness and world generation.
    pub seed: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 30.0,
            max_ticks_per_frame: 4,
            seed: 0x00C0_FFEE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Wait for vertical sync. Disable to measure uncapped FPS.
    pub vsync: bool,
    /// Render distance in chunks.
    pub render_distance_chunks: u32,
    /// Capacity of the GPU brick pool, in 8³ bricks (1 KiB each).
    pub brick_pool_capacity: u32,
    /// Vertical field of view in degrees.
    pub fov_y_deg: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            render_distance_chunks: 16,
            brick_pool_capacity: 65_536,
            fov_y_deg: 70.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlsConfig {
    /// Fly speed in metres per second (1 m = 16 microvoxels).
    pub fly_speed: f32,
    /// Speed multiplier while holding the "fast" key.
    pub fast_multiplier: f32,
    /// Radians of rotation per pixel of mouse motion.
    pub mouse_sensitivity: f32,
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            fly_speed: 4.0,
            fast_multiplier: 5.0,
            mouse_sensitivity: 0.0025,
        }
    }
}

/// Removes a UTF-8 byte order mark, which some Windows editors add and RON rejects.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

impl Config {
    /// Parses and validates a configuration from RON text.
    pub fn from_ron(text: &str, origin: &str) -> Result<Self, ConfigError> {
        let config: Self = ron::from_str(strip_bom(text)).map_err(|source| ConfigError::Parse {
            path: origin.to_owned(),
            source: Box::new(source),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Loads `config.ron` from `assets_dir`. A missing file yields the defaults;
    /// a malformed or invalid file is an error.
    pub fn load(assets_dir: Option<&Path>) -> Result<Self, ConfigError> {
        let Some(path) = assets_dir.map(|dir| dir.join(CONFIG_FILE)) else {
            tracing::warn!("assets directory not found; using default config");
            return Ok(Self::default());
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("{} not found; using default config", path.display());
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        Self::from_ron(&text, &path.display().to_string())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |field, reason: &str| {
            Err(ConfigError::Invalid {
                field,
                reason: reason.to_owned(),
            })
        };
        if !(self.sim.tick_rate_hz.is_finite() && self.sim.tick_rate_hz > 0.0) {
            return invalid("sim.tick_rate_hz", "must be a positive number");
        }
        if self.sim.max_ticks_per_frame == 0 {
            return invalid("sim.max_ticks_per_frame", "must be at least 1");
        }
        if !(1.0..=170.0).contains(&self.render.fov_y_deg) {
            return invalid("render.fov_y_deg", "must be between 1 and 170");
        }
        if self.render.brick_pool_capacity == 0 {
            return invalid("render.brick_pool_capacity", "must be at least 1");
        }
        if self.window.width == 0 || self.window.height == 0 {
            return invalid("window.width/height", "must be non-zero");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::from_ron("()", "test").unwrap(), Config::default());
    }

    #[test]
    fn partial_override() {
        let c = Config::from_ron("(sim: (tick_rate_hz: 60.0))", "test").unwrap();
        assert_eq!(c.sim.tick_rate_hz, 60.0);
        assert_eq!(
            c.sim.max_ticks_per_frame,
            SimConfig::default().max_ticks_per_frame
        );
        assert_eq!(c.window, WindowConfig::default());
    }

    #[test]
    fn accepts_utf8_bom() {
        let c = Config::from_ron("\u{feff}(sim: (seed: 7))", "test").unwrap();
        assert_eq!(c.sim.seed, 7);
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(matches!(
            Config::from_ron("(sim: (tick_rate: 60.0))", "test"),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(matches!(
            Config::from_ron("(sim: (tick_rate_hz: 0.0))", "test"),
            Err(ConfigError::Invalid { .. })
        ));
    }

    #[test]
    fn shipped_config_is_valid() {
        let text = include_str!("../../../assets/config.ron");
        Config::from_ron(text, "assets/config.ron").expect("assets/config.ron must be valid");
    }
}
