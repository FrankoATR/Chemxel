//! Chemical reactions between neighbouring voxels (PROMPT.md §6.5).
//!
//! Loaded from `assets/reactions/*.ron` after the materials, so names can be
//! resolved. A reaction is directional: it is evaluated while the simulation scans a
//! voxel of `a` and finds a face neighbour of `b`. Lookup is O(1) through a dense
//! `a × b` matrix.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{MaterialId, MaterialRegistry, RegistryError};

/// One entry of a `assets/reactions/*.ron` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename = "Reaction")]
struct ReactionDef {
    a: String,
    b: String,
    /// What `a` becomes (default: unchanged).
    #[serde(default)]
    result_a: Option<String>,
    /// What `b` becomes (default: unchanged).
    #[serde(default)]
    result_b: Option<String>,
    /// Probability per tick and contact.
    probability: f32,
    /// Energy released (> 0) or absorbed (< 0), in `density × heat_capacity × K`.
    #[serde(default)]
    heat: f32,
    /// Required temperature range of `a`'s cell (°C, inclusive).
    #[serde(default)]
    min_temp: Option<f32>,
    #[serde(default)]
    max_temp: Option<f32>,
}

/// A resolved reaction `a + b → result_a + result_b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reaction {
    pub a: MaterialId,
    pub b: MaterialId,
    pub result_a: MaterialId,
    pub result_b: MaterialId,
    pub probability: f32,
    pub heat: f32,
    pub min_temp: f32,
    pub max_temp: f32,
}

/// Every reaction plus an `a × b` lookup matrix.
#[derive(Debug, Clone, Default)]
pub struct ReactionSet {
    reactions: Vec<Reaction>,
    /// Number of materials (matrix side).
    n: usize,
    /// `pairs[a * n + b]` = index + 1 into `reactions`, or 0.
    pairs: Vec<u16>,
}

impl ReactionSet {
    /// Loads every `*.ron` file in `dir` (file-name order). A missing directory
    /// means no reactions.
    pub fn load_dir(dir: &Path, materials: &MaterialRegistry) -> Result<Self, RegistryError> {
        if !dir.is_dir() {
            tracing::warn!("no reactions directory at {}", dir.display());
            return Ok(Self::empty(materials));
        }
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
        let set = Self::from_sources(
            sources.iter().map(|(o, t)| (o.as_str(), t.as_str())),
            materials,
        )?;
        tracing::info!("loaded {} reactions from {}", set.len(), dir.display());
        Ok(set)
    }

    /// No reactions.
    pub fn empty(materials: &MaterialRegistry) -> Self {
        Self {
            reactions: Vec::new(),
            n: materials.len(),
            pairs: vec![0; materials.len() * materials.len()],
        }
    }

    /// Builds the set from `(origin, ron_text)` pairs.
    pub fn from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
        materials: &MaterialRegistry,
    ) -> Result<Self, RegistryError> {
        let options = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut set = Self::empty(materials);
        for (origin, text) in sources {
            let text = text.strip_prefix('\u{feff}').unwrap_or(text);
            let defs: Vec<ReactionDef> =
                options
                    .from_str(text)
                    .map_err(|source| RegistryError::Parse {
                        origin: origin.to_owned(),
                        source: Box::new(source),
                    })?;
            for d in defs {
                let r = resolve(&d, origin, materials)?;
                set.insert(r, origin)?;
            }
        }
        Ok(set)
    }

    fn insert(&mut self, r: Reaction, origin: &str) -> Result<(), RegistryError> {
        let slot = r.a.index() * self.n + r.b.index();
        if self.pairs[slot] != 0 {
            return Err(RegistryError::DuplicateReaction {
                origin: origin.to_owned(),
                a: r.a.0,
                b: r.b.0,
            });
        }
        if self.reactions.len() >= usize::from(u16::MAX) {
            return Err(RegistryError::TooMany(self.reactions.len() + 1));
        }
        self.reactions.push(r);
        self.pairs[slot] = self.reactions.len() as u16;
        Ok(())
    }

    /// The reaction started by a voxel of `a` touching one of `b`.
    #[inline]
    pub fn get(&self, a: MaterialId, b: MaterialId) -> Option<&Reaction> {
        let (a, b) = (a.index(), b.index());
        if a >= self.n || b >= self.n {
            return None;
        }
        match self.pairs[a * self.n + b] {
            0 => None,
            i => Some(&self.reactions[usize::from(i) - 1]),
        }
    }

    /// Whether `m` starts any reaction.
    pub fn starts_any(&self, m: MaterialId) -> bool {
        let a = m.index();
        a < self.n
            && self.pairs[a * self.n..(a + 1) * self.n]
                .iter()
                .any(|&i| i != 0)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Reaction> {
        self.reactions.iter()
    }

    pub fn len(&self) -> usize {
        self.reactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reactions.is_empty()
    }

    /// Number of materials the matrix was built for.
    pub fn material_count(&self) -> usize {
        self.n
    }
}

fn resolve(
    d: &ReactionDef,
    origin: &str,
    materials: &MaterialRegistry,
) -> Result<Reaction, RegistryError> {
    let label = format!("{} + {}", d.a, d.b);
    let id = |name: &str, field: &'static str| {
        materials
            .id(name)
            .ok_or_else(|| RegistryError::UnknownReference {
                material: format!("reaction {label}"),
                origin: origin.to_owned(),
                field,
                target: name.to_owned(),
            })
    };
    let a = id(&d.a, "a")?;
    let b = id(&d.b, "b")?;
    let result_a = d.result_a.as_deref().map_or(Ok(a), |n| id(n, "result_a"))?;
    let result_b = d.result_b.as_deref().map_or(Ok(b), |n| id(n, "result_b"))?;
    if !(0.0..=1.0).contains(&d.probability) {
        return Err(RegistryError::InvalidValue {
            what: format!("reaction {label}"),
            origin: origin.to_owned(),
            field: "probability",
            reason: "must be within 0..=1",
        });
    }
    let (min_temp, max_temp) = (
        d.min_temp.unwrap_or(f32::NEG_INFINITY),
        d.max_temp.unwrap_or(f32::INFINITY),
    );
    if min_temp > max_temp {
        return Err(RegistryError::InvalidValue {
            what: format!("reaction {label}"),
            origin: origin.to_owned(),
            field: "min_temp",
            reason: "must not exceed max_temp",
        });
    }
    Ok(Reaction {
        a,
        b,
        result_a,
        result_b,
        probability: d.probability,
        heat: d.heat,
        min_temp,
        max_temp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATERIALS: &str = r#"[
        Material(name: "air",      state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
        Material(name: "water",    state: Liquid, density: 1000, color: (0, 0, 255, 255)),
        Material(name: "lava",     state: Liquid, density: 3100, color: (255, 0, 0, 255)),
        Material(name: "steam",    state: Gas,    density: 0.6,  color: (9, 9, 9, 9)),
        Material(name: "obsidian", state: Solid,  density: 2400, color: (1, 1, 1, 255)),
    ]"#;

    fn materials() -> MaterialRegistry {
        MaterialRegistry::from_sources([("m.ron", MATERIALS)]).unwrap()
    }

    fn load(text: &str) -> Result<ReactionSet, RegistryError> {
        ReactionSet::from_sources([("r.ron", text)], &materials())
    }

    #[test]
    fn lookup_is_directional_and_defaults_keep_materials() {
        let reg = materials();
        let set = load(
            r#"[
                Reaction(a: "water", b: "lava", result_a: "steam", result_b: "obsidian",
                         probability: 0.3, heat: -200),
                Reaction(a: "steam", b: "air", probability: 0.5, min_temp: 50),
            ]"#,
        )
        .unwrap();
        let id = |n| reg.id(n).unwrap();
        let r = set.get(id("water"), id("lava")).unwrap();
        assert_eq!((r.result_a, r.result_b), (id("steam"), id("obsidian")));
        assert_eq!(r.heat, -200.0);
        assert!(set.get(id("lava"), id("water")).is_none(), "directional");
        let s = set.get(id("steam"), id("air")).unwrap();
        assert_eq!((s.result_a, s.result_b), (id("steam"), id("air")));
        assert_eq!((s.min_temp, s.max_temp), (50.0, f32::INFINITY));
        assert!(set.starts_any(id("water")) && !set.starts_any(id("lava")));
    }

    #[test]
    fn unknown_names_are_reported() {
        let err = load(r#"[Reaction(a: "water", b: "acid", probability: 0.1)]"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("acid") && msg.contains("`b`"), "{msg}");
        let err = load(r#"[Reaction(a: "water", b: "lava", result_b: "glass", probability: 1)]"#)
            .unwrap_err();
        assert!(err.to_string().contains("result_b"), "{err}");
    }

    #[test]
    fn invalid_values_and_duplicates_are_errors() {
        assert!(matches!(
            load(r#"[Reaction(a: "water", b: "lava", probability: 1.5)]"#),
            Err(RegistryError::InvalidValue { .. })
        ));
        assert!(matches!(
            load(
                r#"[Reaction(a: "water", b: "lava", probability: 0.1, min_temp: 9, max_temp: 1)]"#
            ),
            Err(RegistryError::InvalidValue { .. })
        ));
        assert!(matches!(
            load(
                r#"[Reaction(a: "water", b: "lava", probability: 0.1),
                    Reaction(a: "water", b: "lava", probability: 0.2)]"#
            ),
            Err(RegistryError::DuplicateReaction { .. })
        ));
        assert!(matches!(
            load(r#"[Reaction(a: "water", b: "lava", probability: 0.1, hot: 1)]"#),
            Err(RegistryError::Parse { .. })
        ));
    }

    #[test]
    fn shipped_reactions_are_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let reg = MaterialRegistry::load_dir(&root.join("materials")).unwrap();
        let set = ReactionSet::load_dir(&root.join("reactions"), &reg)
            .expect("assets/reactions must be valid");
        let id = |n| reg.id(n).unwrap();
        let r = set
            .get(id("water"), id("lava"))
            .expect("water + lava reacts");
        assert_eq!(r.result_b, id("obsidian"));
    }
}
