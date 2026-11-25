//! On-disk (RON) representation of materials. Names are resolved to ids by the
//! registry; nothing outside this crate sees these types.

use serde::Deserialize;

use crate::MatterState;

/// One entry of a `assets/materials/*.ron` file. The Rust name must be `Material`
/// because RON checks the struct name written in the file (`Material(...)`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Material {
    pub name: String,
    pub state: MatterState,
    /// Inventory category (default: from the state, e.g. "Liquids").
    #[serde(default)]
    pub category: Option<String>,
    /// kg/m³. Decides who sinks and who floats.
    pub density: f32,
    /// sRGB colour with alpha (0 = fully transparent).
    pub color: (u8, u8, u8, u8),
    /// W/(m·K).
    #[serde(default)]
    pub conductivity: f32,
    /// kJ/(kg·K).
    #[serde(default = "default_heat_capacity")]
    pub heat_capacity: f32,
    /// Latent energy stored per voxel, relative to the material's lowest phase, in the
    /// same units as `density × heat_capacity` per kelvin. A transition releases or
    /// absorbs the difference between the two materials' values.
    #[serde(default)]
    pub latent_heat: f32,
    /// Heat lost to the environment per voxel, tick and kelvin above ambient
    /// (convection/radiation that the grid does not model; mostly air).
    #[serde(default)]
    pub heat_loss: f32,
    /// Emitted light intensity (0 = none).
    #[serde(default)]
    pub emissive: f32,
    /// Max horizontal cells a liquid/gas spreads per tick.
    #[serde(default)]
    pub dispersion: u8,
    /// Random lifetime range in ticks; the voxel decays when it expires.
    #[serde(default)]
    pub lifetime: Option<(u32, u32)>,
    /// What the voxel becomes when its lifetime expires (default: air).
    #[serde(default)]
    pub decays_into: Option<String>,
    /// Liquids: what a loose surface voxel (a drop, a bump, the rim of a hole in an
    /// incomplete top layer) turns into while at rest, and the chance per tick.
    #[serde(default)]
    pub evaporates_into: Option<String>,
    #[serde(default)]
    pub evaporate_rate: f32,
    /// Gases: at or above this world height (voxels) the voxel turns into
    /// `ceiling_into` (vapour becoming cloud).
    #[serde(default)]
    pub ceiling: Option<i32>,
    #[serde(default)]
    pub ceiling_into: Option<String>,
    /// Gases: whether the voxel moves at all (false: a still cloud).
    #[serde(default = "default_true")]
    pub drifts: bool,
    /// Gases: whether it drifts sideways when it cannot rise (false: vapour trapped
    /// under a roof stays put instead of wandering).
    #[serde(default = "default_true")]
    pub spreads: bool,
    /// Temperature the material is created at and tends to (°C).
    #[serde(default)]
    pub base_temperature: Option<f32>,

    #[serde(default)]
    pub melt_at: Option<f32>,
    #[serde(default)]
    pub melts_into: Option<String>,
    #[serde(default)]
    pub boil_at: Option<f32>,
    #[serde(default)]
    pub boils_into: Option<String>,
    #[serde(default)]
    pub freeze_at: Option<f32>,
    #[serde(default)]
    pub freezes_into: Option<String>,

    #[serde(default)]
    pub flammable: bool,
    #[serde(default)]
    pub ignite_at: Option<f32>,
    #[serde(default)]
    pub burns_into: Option<String>,
    /// Probability per tick that a burning voxel is consumed.
    #[serde(default)]
    pub burn_rate: f32,
    /// Material a burning voxel emits into an adjacent air voxel (e.g. "fire"). Its
    /// `base_temperature` also caps the temperature combustion can reach.
    #[serde(default)]
    pub flame: Option<String>,
    /// Probability per tick that a burning voxel emits a flame.
    #[serde(default)]
    pub flame_rate: f32,
}

fn default_heat_capacity() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}
