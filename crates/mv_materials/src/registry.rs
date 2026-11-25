//! Runtime material table.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::def;

/// Index of a material in the [`MaterialRegistry`]. `0` is always air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[repr(transparent)]
pub struct MaterialId(pub u16);

impl MaterialId {
    pub const AIR: Self = Self(0);

    #[inline]
    pub const fn is_air(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Physical behaviour class of a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum MatterState {
    /// Static; only moves when detached by structural integrity.
    Solid,
    /// Falls and piles up (sand, ash).
    Powder,
    /// Falls and spreads horizontally (water, lava).
    Liquid,
    /// Rises and spreads (steam, smoke).
    Gas,
    /// Consumed over time and spreads heat (fire).
    Energy,
}

/// A temperature-triggered change into another material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    /// Threshold temperature in °C.
    pub at: f32,
    pub into: MaterialId,
}

/// Resolved material properties. See the crate README for units and meaning.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    pub state: MatterState,
    /// Inventory category.
    pub category: String,
    pub density: f32,
    /// sRGB colour with alpha.
    pub color: [u8; 4],
    pub conductivity: f32,
    pub heat_capacity: f32,
    /// Latent energy per voxel relative to the lowest phase (see the README).
    pub latent_heat: f32,
    /// Heat lost to the environment per voxel, tick and kelvin above ambient.
    pub heat_loss: f32,
    pub emissive: f32,
    pub dispersion: u8,
    pub lifetime: Option<(u32, u32)>,
    /// Product when the lifetime expires (air if not given).
    pub decays_into: MaterialId,
    /// What a loose surface voxel of this liquid turns into at rest, and the chance
    /// per tick (0: never).
    pub evaporates_into: Option<MaterialId>,
    pub evaporate_rate: f32,
    /// World height (voxels) at which a gas turns into another material.
    pub ceiling: Option<Transition>,
    /// Whether a gas moves at all, and whether it drifts sideways when it cannot rise.
    pub drifts: bool,
    pub spreads: bool,
    /// Solids the player walks through (vegetation).
    pub passable: bool,
    pub base_temperature: Option<f32>,
    /// Chance per tick of its phase changes (overrides the simulation default).
    pub transition_rate: Option<f32>,
    pub melt: Option<Transition>,
    pub boil: Option<Transition>,
    pub freeze: Option<Transition>,
    pub flammable: bool,
    pub ignite: Option<Transition>,
    pub burn_rate: f32,
    /// Emitted into adjacent air while burning.
    pub flame: Option<MaterialId>,
    pub flame_rate: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {origin}: {source}")]
    Parse {
        origin: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },
    #[error("no material named \"air\" was defined (it is required and gets id 0)")]
    MissingAir,
    #[error("material \"{name}\" is defined twice ({first} and {second})")]
    Duplicate {
        name: String,
        first: String,
        second: String,
    },
    #[error(
        "material \"{material}\" ({origin}): `{field}` references unknown material \"{target}\""
    )]
    UnknownReference {
        material: String,
        origin: String,
        field: &'static str,
        target: String,
    },
    #[error("material \"{material}\" ({origin}): `{set}` and `{missing}` must be given together")]
    IncompleteTransition {
        material: String,
        origin: String,
        set: &'static str,
        missing: &'static str,
    },
    #[error("{what} ({origin}): invalid `{field}`: {reason}")]
    InvalidValue {
        what: String,
        origin: String,
        field: &'static str,
        reason: &'static str,
    },
    #[error("{origin}: reaction between material ids {a} and {b} is defined twice")]
    DuplicateReaction { origin: String, a: u16, b: u16 },
    #[error("too many entries: {0} (max {max})", max = u16::MAX)]
    TooMany(usize),
}

/// Contiguous table of every material, indexed by [`MaterialId`].
#[derive(Debug, Clone)]
pub struct MaterialRegistry {
    materials: Vec<Material>,
    by_name: HashMap<String, MaterialId>,
}

impl MaterialRegistry {
    /// Loads every `*.ron` file in `dir`, in file-name order (ids are deterministic).
    pub fn load_dir(dir: &Path) -> Result<Self, RegistryError> {
        let io_err = |source| RegistryError::Io {
            path: dir.to_owned(),
            source,
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(io_err)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "ron"))
            .collect();
        files.sort();

        let mut sources = Vec::with_capacity(files.len());
        for path in files {
            let text = std::fs::read_to_string(&path).map_err(|source| RegistryError::Io {
                path: path.clone(),
                source,
            })?;
            sources.push((path.display().to_string(), text));
        }
        let registry = Self::from_sources(sources.iter().map(|(o, t)| (o.as_str(), t.as_str())))?;
        tracing::info!("loaded {} materials from {}", registry.len(), dir.display());
        Ok(registry)
    }

    /// Builds a registry from `(origin, ron_text)` pairs. `origin` is only used in
    /// error messages.
    pub fn from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RegistryError> {
        let options = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut defs: Vec<(String, def::Material)> = Vec::new();
        for (origin, text) in sources {
            // Some Windows editors add a UTF-8 byte order mark, which RON rejects.
            let text = text.strip_prefix('\u{feff}').unwrap_or(text);
            let parsed: Vec<def::Material> =
                options
                    .from_str(text)
                    .map_err(|source| RegistryError::Parse {
                        origin: origin.to_owned(),
                        source: Box::new(source),
                    })?;
            defs.extend(parsed.into_iter().map(|d| (origin.to_owned(), d)));
        }
        Self::from_defs(defs)
    }

    fn from_defs(mut defs: Vec<(String, def::Material)>) -> Result<Self, RegistryError> {
        // Air always gets id 0 so that zero-initialised voxel arrays mean "empty".
        let air = defs
            .iter()
            .position(|(_, d)| d.name == "air")
            .ok_or(RegistryError::MissingAir)?;
        let air_def = defs.remove(air);
        defs.insert(0, air_def);

        if defs.len() > usize::from(u16::MAX) {
            return Err(RegistryError::TooMany(defs.len()));
        }

        let mut by_name = HashMap::with_capacity(defs.len());
        let mut origins: Vec<&str> = Vec::with_capacity(defs.len());
        for (i, (origin, d)) in defs.iter().enumerate() {
            if let Some(prev) = by_name.insert(d.name.clone(), MaterialId(i as u16)) {
                return Err(RegistryError::Duplicate {
                    name: d.name.clone(),
                    first: origins[prev.index()].to_owned(),
                    second: origin.clone(),
                });
            }
            origins.push(origin);
        }

        let materials = defs
            .iter()
            .map(|(origin, d)| resolve(d, origin, &by_name))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { materials, by_name })
    }

    /// Properties of `id`.
    ///
    /// # Panics
    /// If `id` does not belong to this registry.
    #[inline]
    pub fn get(&self, id: MaterialId) -> &Material {
        &self.materials[id.index()]
    }

    pub fn id(&self, name: &str) -> Option<MaterialId> {
        self.by_name.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Checks that every material of `old` keeps its id here, so a world filled with
    /// `old`'s ids stays valid (hot reload may only change properties or append new
    /// materials). Returns the first offending name.
    pub fn keeps_ids_of(&self, old: &MaterialRegistry) -> Result<(), String> {
        for (id, m) in old.iter() {
            if self.id(&m.name) != Some(id) {
                return Err(m.name.clone());
            }
        }
        Ok(())
    }

    /// Iterates `(id, material)` in id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (MaterialId, &Material)> {
        self.materials
            .iter()
            .enumerate()
            .map(|(i, m)| (MaterialId(i as u16), m))
    }
}

fn resolve(
    d: &def::Material,
    origin: &str,
    by_name: &HashMap<String, MaterialId>,
) -> Result<Material, RegistryError> {
    let transition = |at: Option<f32>,
                      into: &Option<String>,
                      at_field: &'static str,
                      into_field: &'static str|
     -> Result<Option<Transition>, RegistryError> {
        match (at, into) {
            (None, None) => Ok(None),
            (Some(at), Some(target)) => {
                let into = by_name.get(target).copied().ok_or_else(|| {
                    RegistryError::UnknownReference {
                        material: d.name.clone(),
                        origin: origin.to_owned(),
                        field: into_field,
                        target: target.clone(),
                    }
                })?;
                Ok(Some(Transition { at, into }))
            }
            (Some(_), None) => Err(RegistryError::IncompleteTransition {
                material: d.name.clone(),
                origin: origin.to_owned(),
                set: at_field,
                missing: into_field,
            }),
            (None, Some(_)) => Err(RegistryError::IncompleteTransition {
                material: d.name.clone(),
                origin: origin.to_owned(),
                set: into_field,
                missing: at_field,
            }),
        }
    };

    let reference = |name: &Option<String>, field: &'static str| {
        name.as_ref()
            .map(|target| {
                by_name
                    .get(target)
                    .copied()
                    .ok_or_else(|| RegistryError::UnknownReference {
                        material: d.name.clone(),
                        origin: origin.to_owned(),
                        field,
                        target: target.clone(),
                    })
            })
            .transpose()
    };
    let invalid = |field, reason| RegistryError::InvalidValue {
        what: format!("material \"{}\"", d.name),
        origin: origin.to_owned(),
        field,
        reason,
    };
    if !(0.0..=1.0).contains(&d.burn_rate) {
        return Err(invalid("burn_rate", "must be within 0..=1"));
    }
    if !(0.0..=1.0).contains(&d.heat_loss) {
        return Err(invalid("heat_loss", "must be within 0..=1"));
    }
    if !(0.0..=1.0).contains(&d.flame_rate) {
        return Err(invalid("flame_rate", "must be within 0..=1"));
    }
    if let Some((lo, hi)) = d.lifetime
        && (lo == 0 || lo > hi)
    {
        return Err(invalid("lifetime", "needs 0 < min <= max"));
    }
    if d.decays_into.is_some() && d.lifetime.is_none() {
        return Err(invalid("decays_into", "needs a lifetime"));
    }
    if d.transition_rate.is_some_and(|r| !(0.0..=1.0).contains(&r)) {
        return Err(invalid("transition_rate", "must be within 0..=1"));
    }
    if !(0.0..=1.0).contains(&d.evaporate_rate) {
        return Err(invalid("evaporate_rate", "must be within 0..=1"));
    }
    if d.evaporates_into.is_some() != (d.evaporate_rate > 0.0) {
        return Err(invalid(
            "evaporates_into",
            "evaporates_into and a positive evaporate_rate go together",
        ));
    }

    let (r, g, b, a) = d.color;
    Ok(Material {
        name: d.name.clone(),
        state: d.state,
        category: d.category.clone().unwrap_or_else(|| {
            match d.state {
                MatterState::Solid => "Solids",
                MatterState::Powder => "Powders",
                MatterState::Liquid => "Liquids",
                MatterState::Gas => "Gases",
                MatterState::Energy => "Energy",
            }
            .to_owned()
        }),
        density: d.density,
        color: [r, g, b, a],
        conductivity: d.conductivity,
        heat_capacity: d.heat_capacity,
        latent_heat: d.latent_heat,
        heat_loss: d.heat_loss,
        emissive: d.emissive,
        dispersion: d.dispersion,
        lifetime: d.lifetime,
        decays_into: reference(&d.decays_into, "decays_into")?.unwrap_or(MaterialId::AIR),
        evaporates_into: reference(&d.evaporates_into, "evaporates_into")?,
        evaporate_rate: d.evaporate_rate,
        ceiling: transition(
            d.ceiling.map(|h| h as f32),
            &d.ceiling_into,
            "ceiling",
            "ceiling_into",
        )?,
        drifts: d.drifts,
        spreads: d.spreads,
        passable: d.passable,
        base_temperature: d.base_temperature,
        transition_rate: d.transition_rate,
        melt: transition(d.melt_at, &d.melts_into, "melt_at", "melts_into")?,
        boil: transition(d.boil_at, &d.boils_into, "boil_at", "boils_into")?,
        freeze: transition(d.freeze_at, &d.freezes_into, "freeze_at", "freezes_into")?,
        flammable: d.flammable,
        ignite: transition(d.ignite_at, &d.burns_into, "ignite_at", "burns_into")?,
        burn_rate: d.burn_rate,
        flame: reference(&d.flame, "flame")?,
        flame_rate: d.flame_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(text: &str) -> Result<MaterialRegistry, RegistryError> {
        MaterialRegistry::from_sources([("test.ron", text)])
    }

    #[test]
    fn air_is_always_id_zero() {
        let reg = load(
            r#"[
                Material(name: "stone", state: Solid, density: 2600, color: (1,2,3,255)),
                Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
            ]"#,
        )
        .unwrap();
        assert_eq!(reg.id("air"), Some(MaterialId::AIR));
        assert_eq!(reg.id("stone"), Some(MaterialId(1)));
        assert_eq!(reg.get(MaterialId(1)).color, [1, 2, 3, 255]);
    }

    #[test]
    fn implicit_some_and_transitions_resolve() {
        let reg = load(
            r#"[
                Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                Material(name: "ice", state: Solid, density: 917, color: (200,220,255,200),
                         melt_at: 0, melts_into: "water"),
                Material(name: "water", state: Liquid, density: 1000, color: (40,90,200,180),
                         dispersion: 5, lifetime: Some((1, 2))),
            ]"#,
        )
        .unwrap();
        let ice = reg.get(reg.id("ice").unwrap());
        assert_eq!(
            ice.melt,
            Some(Transition {
                at: 0.0,
                into: reg.id("water").unwrap()
            })
        );
        assert_eq!(reg.get(reg.id("water").unwrap()).lifetime, Some((1, 2)));
    }

    #[test]
    fn missing_air_is_an_error() {
        let err = load(r#"[Material(name: "stone", state: Solid, density: 1, color: (0,0,0,0))]"#);
        assert!(matches!(err, Err(RegistryError::MissingAir)));
    }

    #[test]
    fn unknown_reference_names_the_culprit() {
        let err = load(
            r#"[
                Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                Material(name: "sand", state: Powder, density: 1600, color: (0,0,0,255),
                         melt_at: 1700, melts_into: "glass"),
            ]"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("sand") && msg.contains("glass") && msg.contains("melts_into"),
            "{msg}"
        );
    }

    #[test]
    fn incomplete_transition_is_an_error() {
        let err = load(
            r#"[
                Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                Material(name: "x", state: Solid, density: 1, color: (0,0,0,0), boil_at: 10),
            ]"#,
        );
        assert!(matches!(
            err,
            Err(RegistryError::IncompleteTransition { .. })
        ));
    }

    #[test]
    fn duplicates_are_an_error() {
        let err = MaterialRegistry::from_sources([
            (
                "a.ron",
                r#"[Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0))]"#,
            ),
            (
                "b.ron",
                r#"[Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0))]"#,
            ),
        ]);
        assert!(matches!(err, Err(RegistryError::Duplicate { .. })));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = load(
            r#"[Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0), colour: 1)]"#,
        );
        assert!(matches!(err, Err(RegistryError::Parse { .. })));
    }

    #[test]
    fn decay_and_flame_references_resolve() {
        let reg = load(
            r#"[
                Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                Material(name: "fire", state: Energy, density: 0.3, color: (9,9,9,9),
                         lifetime: (5, 10), decays_into: "smoke"),
                Material(name: "smoke", state: Gas, density: 0.9, color: (9,9,9,9),
                         lifetime: (5, 10)),
                Material(name: "wood", state: Solid, density: 700, color: (9,9,9,9),
                         flammable: true, ignite_at: 300, burns_into: "air",
                         flame: "fire", flame_rate: 0.1),
            ]"#,
        )
        .unwrap();
        let id = |n| reg.id(n).unwrap();
        assert_eq!(
            reg.get(id("fire")).category,
            "Energy",
            "default from the state"
        );
        assert_eq!(reg.get(id("fire")).decays_into, id("smoke"));
        assert_eq!(reg.get(id("smoke")).decays_into, MaterialId::AIR);
        assert_eq!(reg.get(id("wood")).flame, Some(id("fire")));
    }

    #[test]
    fn invalid_rates_and_lifetimes_are_errors() {
        for bad in [
            r#"Material(name: "x", state: Solid, density: 1, color: (0,0,0,0), burn_rate: 2)"#,
            r#"Material(name: "x", state: Gas, density: 1, color: (0,0,0,0), lifetime: (5, 2))"#,
            r#"Material(name: "x", state: Gas, density: 1, color: (0,0,0,0), decays_into: "air")"#,
        ] {
            let text = format!(
                r#"[Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)), {bad}]"#
            );
            assert!(
                matches!(load(&text), Err(RegistryError::InvalidValue { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn id_compatibility_for_hot_reload() {
        let base = r#"Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                      Material(name: "a", state: Solid, density: 1, color: (0,0,0,0)),"#;
        let old = load(&format!("[{base}]")).unwrap();
        let appended = load(&format!(
            r#"[{base} Material(name: "b", state: Solid, density: 1, color: (0,0,0,0))]"#
        ))
        .unwrap();
        assert!(appended.keeps_ids_of(&old).is_ok());
        let reordered = load(
            r#"[Material(name: "air", state: Gas, density: 1.2, color: (0,0,0,0)),
                Material(name: "b", state: Solid, density: 1, color: (0,0,0,0)),
                Material(name: "a", state: Solid, density: 1, color: (0,0,0,0))]"#,
        )
        .unwrap();
        assert_eq!(reordered.keeps_ids_of(&old), Err("a".to_owned()));
    }

    #[test]
    fn shipped_materials_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
        let reg = MaterialRegistry::load_dir(&dir).expect("assets/materials must be valid");
        for name in ["air", "stone", "dirt", "grass", "sand", "water", "lava"] {
            assert!(reg.id(name).is_some(), "missing material {name}");
        }
    }
}
