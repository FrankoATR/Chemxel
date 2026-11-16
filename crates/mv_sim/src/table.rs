//! Per-material properties the automaton needs, in a flat table indexed by
//! `MaterialId` (one array access per voxel, no pointer chasing).

use mv_materials::{MaterialId, MaterialRegistry, MatterState};

/// Movement class of a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Never moves on its own (solids; energy until milestone 5).
    Static,
    Powder,
    Liquid,
    Gas,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Props {
    pub kind: Kind,
    pub density: f32,
    /// Max horizontal cells a liquid (or gas) travels in one tick (at least 1).
    pub dispersion: u8,
}

/// A temperature-triggered change into another material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub at: f32,
    pub into: MaterialId,
}

/// Thermal properties (ADR 007).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThermalProps {
    /// Heat capacity of one voxel: `density × heat_capacity` (energy per kelvin).
    pub capacity: f32,
    pub conductivity: f32,
    /// Latent energy per voxel relative to the lowest phase.
    pub latent: f32,
    /// Temperature the material is created at by edits.
    pub base_temperature: Option<f32>,
    /// Changes above a temperature, highest threshold first (boil before melt).
    pub up: [Option<Change>; 2],
    /// Change below a temperature (freeze / condense).
    pub down: Option<Change>,
}

impl ThermalProps {
    pub fn has_changes(&self) -> bool {
        self.up[0].is_some() || self.down.is_some()
    }

    /// The material this one turns into at temperature `t`, with `hysteresis`.
    #[inline]
    pub fn change_at(&self, t: f32, hysteresis: f32) -> Option<MaterialId> {
        for c in self.up.iter().flatten() {
            if t > c.at + hysteresis {
                return Some(c.into);
            }
        }
        self.down.filter(|c| t < c.at - hysteresis).map(|c| c.into)
    }
}

#[derive(Debug, Clone)]
pub struct MaterialTable {
    props: Vec<Props>,
    thermal: Vec<ThermalProps>,
}

impl MaterialTable {
    pub fn from_registry(registry: &MaterialRegistry) -> Self {
        let props = registry
            .iter()
            .map(|(_, m)| Props {
                kind: match m.state {
                    MatterState::Solid | MatterState::Energy => Kind::Static,
                    MatterState::Powder => Kind::Powder,
                    MatterState::Liquid => Kind::Liquid,
                    MatterState::Gas => Kind::Gas,
                },
                density: m.density,
                dispersion: m.dispersion.max(1),
            })
            .collect();
        let thermal = registry
            .iter()
            .map(|(_, m)| {
                let change = |t: Option<mv_materials::Transition>| {
                    t.map(|t| Change {
                        at: t.at,
                        into: t.into,
                    })
                };
                let mut up = [change(m.boil), change(m.melt)];
                if up[0].is_none() {
                    up.swap(0, 1);
                }
                if let [Some(a), Some(b)] = up
                    && a.at < b.at
                {
                    up = [Some(b), Some(a)];
                }
                ThermalProps {
                    // Strictly positive so cell temperatures are always defined.
                    capacity: (m.density * m.heat_capacity).max(1e-3),
                    conductivity: m.conductivity.max(0.0),
                    latent: m.latent_heat,
                    base_temperature: m.base_temperature,
                    up,
                    down: change(m.freeze),
                }
            })
            .collect();
        Self { props, thermal }
    }

    /// Thermal properties of `id`.
    #[inline]
    pub fn thermal(&self, id: MaterialId) -> &ThermalProps {
        &self.thermal[id.index().min(self.thermal.len() - 1)]
    }

    /// Properties of `id`. Unknown ids behave as static solids.
    #[inline]
    pub fn get(&self, id: MaterialId) -> Props {
        self.props.get(id.index()).copied().unwrap_or(Props {
            kind: Kind::Static,
            density: f32::MAX,
            dispersion: 1,
        })
    }
}
