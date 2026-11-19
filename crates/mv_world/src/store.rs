//! Saved chunks on disk (ADR 011): one file per modified chunk in a directory,
//! named `x_y_z.cxc`, in the [`persist`](crate::persist) format.
//!
//! Writes go to a temporary file renamed over the old one, so a crash mid-save
//! never leaves a half-written chunk. The set of saved positions is listed once at
//! open, so streaming knows without touching the disk whether to load or generate.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mv_core::ChunkPos;
use mv_materials::MaterialRegistry;

use crate::persist::{decode_chunk, encode_chunk};
use crate::{Chunk, MaterialId};

#[derive(Debug)]
pub struct ChunkStore {
    dir: PathBuf,
    /// Material name by id, and back.
    names: Vec<String>,
    ids: HashMap<String, MaterialId>,
    saved: Mutex<HashSet<ChunkPos>>,
}

impl ChunkStore {
    /// Opens (creating it if needed) the chunk directory `dir`.
    pub fn open(dir: &Path, registry: &MaterialRegistry) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut saved = HashSet::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if let Some(pos) = parse_name(&path) {
                saved.insert(pos);
            }
        }
        Ok(Self {
            dir: dir.to_owned(),
            names: registry.iter().map(|(_, m)| m.name.clone()).collect(),
            ids: registry
                .iter()
                .map(|(id, m)| (m.name.clone(), id))
                .collect(),
            saved: Mutex::new(saved),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<ChunkPos>> {
        self.saved.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a saved copy of `pos` exists.
    pub fn contains(&self, pos: ChunkPos) -> bool {
        self.lock().contains(&pos)
    }

    /// Number of saved chunks.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn path(&self, pos: ChunkPos) -> PathBuf {
        let p = pos.0;
        self.dir.join(format!("{}_{}_{}.cxc", p.x, p.y, p.z))
    }

    /// Loads the saved copy of `pos`, if any. A damaged file is reported and
    /// ignored (the chunk is regenerated).
    pub fn load(&self, pos: ChunkPos) -> Option<Chunk> {
        if !self.contains(pos) {
            return None;
        }
        let path = self.path(pos);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("could not read {}: {e}", path.display());
                return None;
            }
        };
        match decode_chunk(&bytes, |name| self.ids.get(name).copied()) {
            Ok(d) => {
                if d.unknown_voxels > 0 {
                    tracing::warn!(
                        "{}: {} voxels of unknown materials loaded as air",
                        path.display(),
                        d.unknown_voxels
                    );
                }
                Some(d.chunk)
            }
            Err(e) => {
                tracing::error!("{}: {e}; regenerating the chunk", path.display());
                None
            }
        }
    }

    /// Writes `chunk` as the saved copy of `pos` and marks it saved.
    pub fn save(&self, pos: ChunkPos, chunk: &mut Chunk) -> io::Result<usize> {
        let bytes = encode_chunk(chunk, |m| {
            self.names.get(m.index()).map_or("air", String::as_str)
        });
        let path = self.path(pos);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.lock().insert(pos);
        chunk.mark_saved();
        Ok(bytes.len())
    }
}

/// `x_y_z.cxc` → chunk position.
fn parse_name(path: &Path) -> Option<ChunkPos> {
    if path.extension()? != "cxc" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let mut it = stem.split('_').map(str::parse::<i32>);
    let (x, y, z) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    it.next().is_none().then(|| ChunkPos::new(x, y, z))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mv_core::LocalPos;

    pub(crate) fn registry() -> MaterialRegistry {
        MaterialRegistry::from_sources([(
            "m.ron",
            r#"[
                Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
                Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
                Material(name: "sand",  state: Powder, density: 1600, color: (2, 2, 2, 255)),
            ]"#,
        )])
        .unwrap()
    }

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chemxel_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = temp_dir("store");
        let reg = registry();
        let store = ChunkStore::open(&dir, &reg).unwrap();
        let pos = ChunkPos::new(-2, 3, -7);
        let mut c = Chunk::uniform(MaterialId(1));
        c.set(LocalPos::new(1, 2, 3), MaterialId(2));
        assert!(c.is_unsaved());
        store.save(pos, &mut c).unwrap();
        assert!(!c.is_unsaved());
        assert!(store.contains(pos));

        // A new store sees the file and loads it back.
        let store = ChunkStore::open(&dir, &reg).unwrap();
        assert_eq!(store.len(), 1);
        let loaded = store.load(pos).unwrap();
        assert_eq!(loaded.get(LocalPos::new(1, 2, 3)), MaterialId(2));
        assert_eq!(loaded.get(LocalPos::new(0, 0, 0)), MaterialId(1));
        assert!(loaded.is_modified() && !loaded.is_unsaved());
        assert!(store.load(ChunkPos::new(0, 0, 0)).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_files_are_regenerated() {
        let dir = temp_dir("store_bad");
        let reg = registry();
        let store = ChunkStore::open(&dir, &reg).unwrap();
        let pos = ChunkPos::new(0, 0, 0);
        store.save(pos, &mut Chunk::uniform(MaterialId(1))).unwrap();
        std::fs::write(dir.join("0_0_0.cxc"), b"garbage").unwrap();
        assert!(store.load(pos).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_names() {
        assert_eq!(
            parse_name(Path::new("a/-1_2_-3.cxc")),
            Some(ChunkPos::new(-1, 2, -3))
        );
        assert_eq!(parse_name(Path::new("1_2.cxc")), None);
        assert_eq!(parse_name(Path::new("1_2_3.tmp")), None);
    }
}
