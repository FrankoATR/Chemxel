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
    pub world: WorldConfig,
    pub controls: ControlsConfig,
    pub editor: EditorConfig,
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
    /// Horizontal render (and load) distance in chunks.
    pub render_distance_chunks: u32,
    /// Vertical render (and load) distance in chunks, above and below the camera.
    pub render_height_chunks: u32,
    /// Capacity of the GPU brick pool, in 8³ bricks (1 KiB each).
    pub brick_pool_capacity: u32,
    /// Max non-uniform chunks resident on the GPU (2 KiB of brick table each).
    pub chunk_slot_capacity: u32,
    /// Max bytes of streamed chunk data uploaded per frame (edits are never delayed).
    pub upload_budget_kib: u32,
    /// Vertical field of view in degrees.
    pub fov_y_deg: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            render_distance_chunks: 16,
            render_height_chunks: 8,
            brick_pool_capacity: 262_144,
            chunk_slot_capacity: 8192,
            upload_budget_kib: 8192,
            fov_y_deg: 70.0,
        }
    }
}

/// World generation and streaming. Distances and heights are in microvoxels unless
/// the name says otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorldConfig {
    /// Water fills every empty voxel below this height.
    pub sea_level: i32,
    pub terrain: TerrainConfig,
    /// Extra chunks beyond the render distance before a chunk is unloaded
    /// (hysteresis, avoids load/unload thrashing at the border).
    pub unload_margin_chunks: u32,
    /// Max chunks being generated in the background at once.
    pub max_inflight_jobs: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            sea_level: 0,
            terrain: TerrainConfig::default(),
            unload_margin_chunks: 2,
            max_inflight_jobs: 64,
        }
    }
}

/// Heightmap terrain: `base + hills + detail + mountains`, each from fBm noise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerrainConfig {
    pub base_height: f64,
    /// Large rolling hills.
    pub hills_amplitude: f64,
    pub hills_wavelength: f64,
    /// Small bumps on top of the hills.
    pub detail_amplitude: f64,
    pub detail_wavelength: f64,
    /// Ridged mountains, masked to appear only in some regions.
    pub mountain_amplitude: f64,
    pub mountain_wavelength: f64,
    pub octaves: u32,
    /// Depth of the dirt layer below the surface.
    pub dirt_depth: i32,
    /// Depth of the grass layer on top of the dirt.
    pub grass_depth: i32,
    /// Columns whose surface is at most this far above sea level become sand.
    pub beach_height: i32,
}

impl Default for TerrainConfig {
    fn default() -> Self {
        Self {
            base_height: 24.0,
            hills_amplitude: 96.0,
            hills_wavelength: 1400.0,
            detail_amplitude: 10.0,
            detail_wavelength: 160.0,
            mountain_amplitude: 320.0,
            mountain_wavelength: 2200.0,
            octaves: 5,
            dirt_depth: 24,
            grass_depth: 2,
            beach_height: 6,
        }
    }
}

/// Voxel editing brush.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// Initial brush radius in microvoxels.
    pub brush_radius: f32,
    /// Largest brush radius selectable.
    pub max_brush_radius: f32,
    /// Max picking distance in microvoxels.
    pub reach: f64,
    /// Brush applications per second while a mouse button is held.
    pub repeat_hz: f32,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            brush_radius: 6.0,
            max_brush_radius: 48.0,
            reach: 1024.0,
            repeat_hz: 12.0,
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
        if self.render.chunk_slot_capacity == 0 {
            return invalid("render.chunk_slot_capacity", "must be at least 1");
        }
        if !(1..=64).contains(&self.render.render_distance_chunks) {
            return invalid("render.render_distance_chunks", "must be between 1 and 64");
        }
        if !(1..=32).contains(&self.render.render_height_chunks) {
            return invalid("render.render_height_chunks", "must be between 1 and 32");
        }
        if self.render.upload_budget_kib == 0 {
            return invalid("render.upload_budget_kib", "must be at least 1");
        }
        if self.world.max_inflight_jobs == 0 {
            return invalid("world.max_inflight_jobs", "must be at least 1");
        }
        let t = &self.world.terrain;
        if !(1..=16).contains(&t.octaves) {
            return invalid("world.terrain.octaves", "must be between 1 and 16");
        }
        if t.hills_wavelength <= 0.0 || t.detail_wavelength <= 0.0 || t.mountain_wavelength <= 0.0 {
            return invalid("world.terrain.*_wavelength", "must be positive");
        }
        if t.dirt_depth < t.grass_depth || t.grass_depth < 0 {
            return invalid("world.terrain.dirt_depth", "must be >= grass_depth >= 0");
        }
        let e = &self.editor;
        if !(e.max_brush_radius >= 1.0 && (0.5..=e.max_brush_radius).contains(&e.brush_radius)) {
            return invalid(
                "editor.brush_radius",
                "must be between 0.5 and max_brush_radius (>= 1)",
            );
        }
        if !(e.reach > 0.0 && e.repeat_hz > 0.0) {
            return invalid("editor.reach/repeat_hz", "must be positive");
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
