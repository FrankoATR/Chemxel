//! Save slots (milestone 8, ADR 011).
//!
//! A world lives in `saves/<name>/`:
//! * `world.ron`: format version, the seed and terrain settings it was generated
//!   with (they override `config.ron`, so saved chunks match the terrain around
//!   them), the simulation tick and where the player was;
//! * `chunks/`: one file per modified chunk ([`mv_world::store`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use mv_core::config::WorldConfig;
use mv_materials::MaterialRegistry;
use mv_world::store::ChunkStore;
use serde::{Deserialize, Serialize};

/// Version of `world.ron`; bump when its meaning changes.
pub const SAVE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    /// Feet position (bottom centre of the player box), in microvoxels.
    pub feet: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub walking: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveHeader {
    pub version: u32,
    pub seed: u64,
    pub world: WorldConfig,
    pub tick: u64,
    pub player: PlayerState,
}

/// An open save slot.
#[derive(Debug)]
pub struct SaveSlot {
    pub name: String,
    dir: PathBuf,
    pub store: Arc<ChunkStore>,
}

impl SaveSlot {
    /// Opens (or creates) the slot `name` under `root`.
    pub fn open(root: &Path, name: &str, registry: &MaterialRegistry) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || "-_".contains(c)),
            "world name \"{name}\" may only contain letters, digits, '-' and '_'"
        );
        let dir = root.join(name);
        let store = ChunkStore::open(&dir.join("chunks"), registry)
            .with_context(|| format!("opening the save {}", dir.display()))?;
        Ok(Self {
            name: name.to_owned(),
            dir,
            store: Arc::new(store),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn header_path(&self) -> PathBuf {
        self.dir.join("world.ron")
    }

    /// The saved header, if this world was saved before.
    pub fn load_header(&self) -> anyhow::Result<Option<SaveHeader>> {
        let path = self.header_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let header: SaveHeader =
            ron::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        anyhow::ensure!(
            header.version <= SAVE_VERSION,
            "{} was written by a newer version of the game (save format {}, this game reads up to {SAVE_VERSION})",
            path.display(),
            header.version
        );
        Ok(Some(header))
    }

    /// Writes the header (atomically).
    pub fn write_header(&self, header: &SaveHeader) -> anyhow::Result<()> {
        let text = ron::ser::to_string_pretty(header, ron::ser::PrettyConfig::default())
            .context("serialising the save header")?;
        let path = self.header_path();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_version_check() {
        let root = std::env::temp_dir().join(format!("chemxel_save_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let registry = MaterialRegistry::from_sources([(
            "m.ron",
            r#"[Material(name: "air", state: Gas, density: 1.2, color: (0, 0, 0, 0))]"#,
        )])
        .unwrap();
        let slot = SaveSlot::open(&root, "test_world", &registry).unwrap();
        assert!(slot.load_header().unwrap().is_none());
        let header = SaveHeader {
            version: SAVE_VERSION,
            seed: 42,
            world: WorldConfig::default(),
            tick: 1234,
            player: PlayerState {
                feet: [1.5, -20.25, 1e6],
                yaw: 0.5,
                pitch: -0.2,
                walking: true,
            },
        };
        slot.write_header(&header).unwrap();
        assert_eq!(slot.load_header().unwrap(), Some(header.clone()));

        let newer = SaveHeader {
            version: SAVE_VERSION + 1,
            ..header
        };
        slot.write_header(&newer).unwrap();
        assert!(slot.load_header().is_err());
        assert!(SaveSlot::open(&root, "../escape", &registry).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
