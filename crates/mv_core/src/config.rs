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
    pub physics: PhysicsConfig,
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
    /// Ticks without changes before an active chunk (or a temperature field that
    /// returned to ambient) falls asleep.
    pub sleep_after_ticks: u32,
    /// Temperature of everything without a temperature field, °C.
    pub ambient_temperature: f32,
    /// Heat diffusion speed (multiplies the harmonic mean of the conductivities).
    pub diffusion_rate: f32,
    /// Probability per tick that a voxel past a transition temperature changes.
    pub transition_rate: f32,
    /// Margin past a transition temperature before it triggers, °C (avoids
    /// melting/freezing back and forth at the threshold).
    pub transition_hysteresis: f32,
    /// A temperature field within this many °C of ambient everywhere may sleep, and
    /// neighbours get a field once a shared face deviates by more.
    pub thermal_epsilon: f32,
    /// Structural integrity: a connected solid bigger than this (voxels) counts as
    /// anchored without exploring further.
    pub integrity_max_voxels: u32,
    /// Same for the bounding box of the component, in voxels per axis.
    pub integrity_max_extent: u32,
    /// Voxels the integrity flood fill may visit per tick; pending checks wait.
    pub integrity_budget_voxels: u32,
    /// Floating pieces smaller than this stay in place instead of becoming rigid
    /// bodies (crumbs left by fire or acid would otherwise flood the physics).
    pub integrity_min_voxels: u32,
    /// Solids lost to reactions, burning or melting are gathered and checked every
    /// this many ticks (edits are checked at once).
    pub integrity_natural_period: u32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 30.0,
            max_ticks_per_frame: 4,
            seed: 0x00C0_FFEE,
            sleep_after_ticks: 2,
            ambient_temperature: 20.0,
            diffusion_rate: 300.0,
            transition_rate: 0.1,
            transition_hysteresis: 2.0,
            thermal_epsilon: 0.5,
            integrity_max_voxels: 65_536,
            integrity_max_extent: 192,
            integrity_budget_voxels: 262_144,
            integrity_min_voxels: 64,
            integrity_natural_period: 8,
        }
    }
}

/// Rigid bodies (milestone 6, ADR 009). Distances in microvoxels unless stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhysicsConfig {
    /// Gravity, m/s².
    pub gravity: f32,
    /// A body slower than this (m/s and rad/s) for `settle_ticks` is put back into
    /// the voxel grid.
    pub settle_speed: f32,
    pub settle_ticks: u32,
    /// Bodies are put back after this many seconds no matter what.
    pub max_body_seconds: f32,
    /// World voxels around a body that get a collider, beyond its bounding box.
    pub world_margin: u32,
    /// Max bodies at once; further detached pieces are put back immediately.
    pub max_bodies: u32,
    /// GPU storage for body voxels (2 bytes each).
    pub max_body_voxels: u32,
    /// Friction and restitution of voxel contacts.
    pub friction: f32,
    pub restitution: f32,
    /// Max physics substeps per tick. A tick is split so no body moves more than
    /// half a voxel per substep (voxel colliders only touch at their surface), and
    /// speeds are capped accordingly.
    pub max_substeps: u32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: 9.81,
            settle_speed: 0.05,
            settle_ticks: 15,
            max_body_seconds: 20.0,
            world_margin: 12,
            max_bodies: 64,
            max_body_voxels: 4 << 20,
            friction: 0.7,
            restitution: 0.05,
            max_substeps: 16,
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
    /// Temperature fields the heat view (F5) can show at once (16 KiB each).
    pub heat_view_capacity: u32,
    /// Vertical field of view in degrees.
    pub fov_y_deg: f32,
    /// Direction towards the sun (any length).
    pub sun_direction: [f32; 3],
    /// Max length of sun shadow rays, in microvoxels (0 disables shadows).
    pub shadow_distance: f32,
    /// Ambient occlusion strength, 0..1 (0 disables it, and the light emissive
    /// voxels cast around them).
    pub ao_strength: f32,
    /// Shading normals smoothed over neighbouring voxels, 0..1 (0 = flat faces, every
    /// voxel reads as a cube; higher = slopes and edges look rounded).
    pub smooth_normals: f32,
    /// Per-voxel brightness noise, 0..1 (1 = strong, every voxel stands out).
    pub color_noise: f32,
    /// Spray, foam and sparks drawn at once (0 turns particles off).
    pub max_particles: u32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            render_distance_chunks: 16,
            render_height_chunks: 8,
            brick_pool_capacity: 393_216,
            chunk_slot_capacity: 8192,
            upload_budget_kib: 8192,
            heat_view_capacity: 1024,
            fov_y_deg: 70.0,
            sun_direction: [0.55, 0.62, 0.32],
            shadow_distance: 768.0,
            ao_strength: 0.8,
            smooth_normals: 0.5,
            color_noise: 0.3,
            max_particles: 8192,
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
    /// Solids deeper than this many voxels below the generated terrain surface are
    /// anchored (structural integrity stops there).
    pub anchor_depth: i32,
    /// Seconds between automatic saves (0 = only on exit).
    pub autosave_seconds: f32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            sea_level: 0,
            terrain: TerrainConfig::default(),
            unload_margin_chunks: 2,
            max_inflight_jobs: 64,
            anchor_depth: 32,
            autosave_seconds: 60.0,
        }
    }
}

/// Heightmap terrain: `base + hills + detail + mountains (+ roughness + cliffs)`,
/// each from fBm noise, with vegetation, trees and boulders on top (generator 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerrainConfig {
    /// Generator version. 1: the plain heightmap of milestone 2 (saves that do not
    /// name a version keep it, so their saved chunks match the terrain around them);
    /// 2: natural landscape (ADR 013).
    #[serde(default = "legacy_generator")]
    pub generator: u32,
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
    /// Generator 2. Fine bumps that break the long contour lines of gentle slopes.
    pub roughness_amplitude: f64,
    pub roughness_wavelength: f64,
    /// Generator 2. Stepped cliffs (total height of the steps), in some regions.
    pub cliff_amplitude: f64,
    pub cliff_wavelength: f64,
    /// Generator 2. Columns this much higher or lower than a neighbour show bare
    /// rock (with strata) instead of grass.
    pub rock_slope: i32,
    /// Generator 2. Share of meadow columns with a tuft of tall grass (0..1).
    pub grass_density: f64,
    /// Generator 2. Share of tufts that carry a flower, inside flower patches (0..1).
    pub flower_chance: f64,
    /// Generator 2. Chance of a tree per `tree_spacing`² cell inside forests (0..1).
    pub tree_density: f64,
    pub tree_spacing: i32,
    /// Generator 2. Chance of a boulder per 96² cell (0..1).
    pub boulder_density: f64,
}

fn legacy_generator() -> u32 {
    1
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
            generator: 2,
            roughness_amplitude: 2.5,
            roughness_wavelength: 18.0,
            cliff_amplitude: 96.0,
            cliff_wavelength: 700.0,
            rock_slope: 4,
            grass_density: 0.3,
            flower_chance: 0.25,
            tree_density: 0.6,
            tree_spacing: 112,
            boulder_density: 0.2,
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
    /// Start walking (true) or flying (false); `F` toggles in game.
    pub start_walking: bool,
    /// Walking speed, m/s (Shift runs at up to 2x).
    pub walk_speed: f32,
    /// Jump height, metres.
    pub jump_height: f32,
    /// Gravity on the player, m/s².
    pub gravity: f32,
    /// Ledges up to this height are climbed automatically, metres.
    pub step_height: f32,
    /// Player box and eye height, metres.
    pub player_width: f32,
    pub player_height: f32,
    pub eye_height: f32,
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            fly_speed: 4.0,
            fast_multiplier: 5.0,
            mouse_sensitivity: 0.0025,
            start_walking: true,
            walk_speed: 4.3,
            jump_height: 1.1,
            gravity: 20.0,
            step_height: 0.5,
            player_width: 0.6,
            player_height: 1.8,
            eye_height: 1.62,
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
        let sim = &self.sim;
        if !(sim.diffusion_rate >= 0.0 && (0.0..=1.0).contains(&sim.transition_rate)) {
            return invalid(
                "sim.diffusion_rate/transition_rate",
                "diffusion_rate must be >= 0 and transition_rate in 0..=1",
            );
        }
        if !(sim.thermal_epsilon > 0.0 && sim.transition_hysteresis >= 0.0) {
            return invalid(
                "sim.thermal_epsilon/transition_hysteresis",
                "epsilon must be > 0 and hysteresis >= 0",
            );
        }
        let r = &self.render;
        if !(r.shadow_distance >= 0.0 && (0.0..=1.0).contains(&r.ao_strength))
            || r.sun_direction.iter().all(|&c| c == 0.0)
        {
            return invalid(
                "render.shadow_distance/ao_strength/sun_direction",
                "shadow_distance >= 0, ao_strength in 0..=1, sun_direction non-zero",
            );
        }
        if !((0.0..=1.0).contains(&r.smooth_normals) && (0.0..=1.0).contains(&r.color_noise)) {
            return invalid("render.smooth_normals/color_noise", "must be within 0..=1");
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
        if sim.integrity_max_voxels == 0
            || sim.integrity_max_extent == 0
            || sim.integrity_budget_voxels < sim.integrity_max_voxels
        {
            return invalid(
                "sim.integrity_*",
                "max_voxels and max_extent must be > 0 and budget_voxels >= max_voxels",
            );
        }
        if sim.integrity_natural_period == 0 {
            return invalid("sim.integrity_natural_period", "must be at least 1");
        }
        let c = &self.controls;
        if !(c.walk_speed > 0.0
            && c.jump_height >= 0.0
            && c.gravity > 0.0
            && c.step_height >= 0.0
            && c.player_width > 0.0
            && c.player_height > c.player_width.min(c.eye_height)
            && c.eye_height > 0.0
            && c.eye_height <= c.player_height)
        {
            return invalid(
                "controls.walk_speed/jump_height/gravity/step_height/player_*",
                "positive sizes and speeds, eye_height <= player_height",
            );
        }
        if self.world.autosave_seconds < 0.0 {
            return invalid("world.autosave_seconds", "must be >= 0");
        }
        if self.world.anchor_depth < 0 {
            return invalid("world.anchor_depth", "must be >= 0");
        }
        let p = &self.physics;
        if !(p.gravity.is_finite()
            && p.settle_speed > 0.0
            && p.max_body_seconds > 0.0
            && p.friction >= 0.0
            && (0.0..=1.0).contains(&p.restitution))
        {
            return invalid(
                "physics.*",
                "gravity finite, settle_speed and max_body_seconds > 0, friction >= 0, restitution in 0..=1",
            );
        }
        if !(1..=64).contains(&p.max_substeps) {
            return invalid("physics.max_substeps", "must be between 1 and 64");
        }
        if p.max_bodies == 0 || p.max_body_voxels == 0 {
            return invalid("physics.max_bodies/max_body_voxels", "must be at least 1");
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
    fn terrain_without_generator_is_the_legacy_one() {
        // Saves from before generator 2 list their terrain without `generator`.
        let c = Config::from_ron("(world: (terrain: (base_height: 24.0)))", "test").unwrap();
        assert_eq!(c.world.terrain.generator, 1);
        assert_eq!(Config::default().world.terrain.generator, 2);
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
