//! Per-material properties the simulation needs, in flat tables indexed by
//! `MaterialId` (one array access per voxel, no pointer chasing), plus the reaction
//! matrix.

use mv_materials::{MaterialId, MaterialRegistry, MatterState, ReactionSet};

/// Movement class of a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Never moves on its own (solids).
    Static,
    Powder,
    Liquid,
    /// Rises and drifts (gases; also energy such as fire, which flickers upwards).
    Gas,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Props {
    pub kind: Kind,
    pub density: f32,
    /// Max horizontal cells a liquid (or gas) travels in one tick (at least 1).
    pub dispersion: u8,
    /// Chance per tick (see [`chance`]) that the voxel decays; 0 = never.
    pub decay: u32,
    /// Product of the decay.
    pub decays_into: MaterialId,
    /// Starts at least one reaction (as `a`).
    pub reactive: bool,
    /// Mask (`mv_world::material_bit`) of the materials it reacts with as `a`.
    pub partners: u64,
    /// Liquids: chance per tick that a loose surface voxel at rest evaporates
    /// (0: never), and into what.
    pub evaporate: u32,
    pub evaporates_into: MaterialId,
    /// Gases: world height at which the voxel turns into `ceiling_into`
    /// (`i64::MAX`: never).
    pub ceiling: i64,
    pub ceiling_into: MaterialId,
    /// Gases: whether it moves at all, and sideways when it cannot rise.
    pub drifts: bool,
    pub spreads: bool,
}

/// A temperature-triggered change into another material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub at: f32,
    pub into: MaterialId,
}

/// Combustion of a flammable material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burn {
    /// Ignition temperature.
    pub at: f32,
    pub into: MaterialId,
    /// Chance per tick of being consumed while burning.
    pub rate: u32,
    /// Emitted into adjacent air while burning.
    pub flame: Option<MaterialId>,
    pub flame_rate: u32,
    /// Combustion heat never raises the cell above this (the flame's temperature).
    pub max_temp: f32,
}

/// Thermal properties (ADR 007).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThermalProps {
    /// Heat capacity of one voxel: `density × heat_capacity` (energy per kelvin).
    pub capacity: f32,
    pub conductivity: f32,
    /// Heat lost to the environment per tick and kelvin above ambient.
    pub loss: f32,
    /// Latent energy per voxel relative to the lowest phase (or combustion energy).
    pub latent: f32,
    /// Temperature the material is created at by edits.
    pub base_temperature: Option<f32>,
    /// Chance per tick of its phase changes, if it overrides the default.
    pub rate: Option<u32>,
    /// Changes above a temperature, highest threshold first (boil before melt).
    pub up: [Option<Change>; 2],
    /// Change below a temperature (freeze / condense).
    pub down: Option<Change>,
    pub burn: Option<Burn>,
}

impl ThermalProps {
    pub fn has_changes(&self) -> bool {
        self.up[0].is_some() || self.down.is_some() || self.burn.is_some()
    }

    /// The material this one turns into at temperature `t`, with `hysteresis`
    /// (phase changes only; combustion is [`ThermalProps::burning_at`]).
    #[inline]
    pub fn change_at(&self, t: f32, hysteresis: f32) -> Option<MaterialId> {
        for c in self.up.iter().flatten() {
            if t > c.at + hysteresis {
                return Some(c.into);
            }
        }
        self.down.filter(|c| t < c.at - hysteresis).map(|c| c.into)
    }

    /// Combustion parameters if the material burns at temperature `t`.
    #[inline]
    pub fn burning_at(&self, t: f32) -> Option<&Burn> {
        self.burn.as_ref().filter(|b| t > b.at)
    }

    /// Whether anything (phase change or combustion) can happen at `t`.
    #[inline]
    pub fn active_at(&self, t: f32, hysteresis: f32) -> bool {
        self.change_at(t, hysteresis).is_some() || self.burning_at(t).is_some()
    }
}

/// A resolved reaction for the hot loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub into_a: MaterialId,
    pub into_b: MaterialId,
    /// Chance per tick and contact.
    pub chance: u32,
    pub min_temp: f32,
    pub max_temp: f32,
    pub heat: f32,
    /// Released heat never lifts a cell above this: the `base_temperature` of `a`
    /// (a flame heats things up to its own temperature at most).
    pub heat_cap: f32,
}

/// Converts a probability to a threshold for `rng::hash(..) < threshold`.
#[inline]
pub fn chance(p: f32) -> u32 {
    (f64::from(p.clamp(0.0, 1.0)) * f64::from(u32::MAX)) as u32
}

#[derive(Debug, Clone)]
pub struct MaterialTable {
    props: Vec<Props>,
    thermal: Vec<ThermalProps>,
    rules: Vec<Rule>,
    /// `pairs[a * n + b]` = index + 1 into `rules`, or 0.
    pairs: Vec<u16>,
    n: usize,
}

impl MaterialTable {
    /// Materials without reactions.
    pub fn from_registry(registry: &MaterialRegistry) -> Self {
        Self::new(registry, &ReactionSet::empty(registry))
    }

    pub fn new(registry: &MaterialRegistry, reactions: &ReactionSet) -> Self {
        let n = registry.len();
        let mut rules = Vec::with_capacity(reactions.len());
        let mut pairs = vec![0u16; n * n];
        for r in reactions.iter() {
            if r.a.index() >= n || r.b.index() >= n {
                continue;
            }
            rules.push(Rule {
                into_a: r.result_a,
                into_b: r.result_b,
                chance: chance(r.probability),
                min_temp: r.min_temp,
                max_temp: r.max_temp,
                heat: r.heat,
                heat_cap: registry.get(r.a).base_temperature.unwrap_or(f32::INFINITY),
            });
            pairs[r.a.index() * n + r.b.index()] = rules.len() as u16;
        }
        let props = registry
            .iter()
            .map(|(id, m)| Props {
                kind: match m.state {
                    MatterState::Solid => Kind::Static,
                    MatterState::Powder => Kind::Powder,
                    MatterState::Liquid => Kind::Liquid,
                    MatterState::Gas | MatterState::Energy => Kind::Gas,
                },
                density: m.density,
                dispersion: m.dispersion.max(1),
                // Stateless lifetime: expire with probability 1 / mean each tick.
                decay: m
                    .lifetime
                    .map_or(0, |(lo, hi)| chance(2.0 / (lo + hi).max(1) as f32).max(1)),
                decays_into: m.decays_into,
                reactive: pairs[id.index() * n..(id.index() + 1) * n]
                    .iter()
                    .any(|&i| i != 0),
                partners: pairs[id.index() * n..(id.index() + 1) * n]
                    .iter()
                    .enumerate()
                    .filter(|&(_, &i)| i != 0)
                    .fold(0, |acc, (b, _)| {
                        acc | mv_world::material_bit(MaterialId(b as u16))
                    }),
                evaporate: m
                    .evaporates_into
                    .map_or(0, |_| chance(m.evaporate_rate).max(1)),
                evaporates_into: m.evaporates_into.unwrap_or(MaterialId::AIR),
                ceiling: m.ceiling.map_or(i64::MAX, |c| c.at as i64),
                ceiling_into: m.ceiling.map_or(id, |c| c.into),
                drifts: m.drifts,
                spreads: m.spreads,
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
                let burn = m.ignite.filter(|_| m.flammable).map(|t| Burn {
                    at: t.at,
                    into: t.into,
                    rate: chance(m.burn_rate),
                    flame: m.flame,
                    flame_rate: chance(m.flame_rate),
                    max_temp: m
                        .flame
                        .and_then(|f| registry.get(f).base_temperature)
                        .unwrap_or(f32::INFINITY),
                });
                ThermalProps {
                    // Strictly positive so cell temperatures are always defined.
                    capacity: (m.density * m.heat_capacity).max(1e-3),
                    conductivity: m.conductivity.max(0.0),
                    loss: m.heat_loss.max(0.0),
                    latent: m.latent_heat,
                    base_temperature: m.base_temperature,
                    rate: m.transition_rate.map(chance),
                    up,
                    down: change(m.freeze),
                    burn,
                }
            })
            .collect();
        Self {
            props,
            thermal,
            rules,
            pairs,
            n,
        }
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
            decay: 0,
            decays_into: id,
            reactive: false,
            partners: 0,
            evaporate: 0,
            evaporates_into: id,
            ceiling: i64::MAX,
            ceiling_into: id,
            drifts: false,
            spreads: false,
        })
    }

    /// The reaction a voxel of `a` starts with a neighbour of `b`.
    #[inline]
    pub fn reaction(&self, a: MaterialId, b: MaterialId) -> Option<&Rule> {
        let (a, b) = (a.index(), b.index());
        if a >= self.n || b >= self.n {
            return None;
        }
        match self.pairs[a * self.n + b] {
            0 => None,
            i => Some(&self.rules[usize::from(i) - 1]),
        }
    }

    /// Number of materials.
    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}
