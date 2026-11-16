//! Chunk streaming: keeps the chunks around a centre loaded and drops far ones.
//!
//! * The load region is a cylinder: horizontal radius `radius`, `vertical_radius`
//!   chunks above and below the centre. Chunks are requested nearest first.
//! * Generation runs on the rayon thread pool; results come back through a channel
//!   and are inserted by [`Streamer::update`] on the calling thread, so the
//!   [`World`] is never shared between threads.
//! * Chunks beyond the region plus `unload_margin` are removed. Modified chunks are
//!   kept in a stash and restored instead of regenerated (persistence to disk is
//!   milestone 8).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use glam::IVec3;
use mv_core::ChunkPos;

use crate::terrain::TerrainGenerator;
use crate::{Chunk, World};

/// Shape of the streamed region, in chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSettings {
    pub radius: u32,
    pub vertical_radius: u32,
    pub unload_margin: u32,
    pub max_inflight: usize,
}

impl StreamSettings {
    fn within(&self, center: ChunkPos, pos: ChunkPos, margin: u32) -> bool {
        let d = pos.0 - center.0;
        let r = i64::from(self.radius + margin);
        let horizontal = i64::from(d.x).pow(2) + i64::from(d.z).pow(2);
        horizontal <= r * r && d.y.unsigned_abs() <= self.vertical_radius + margin
    }
}

/// Counters for the debug overlay.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamStats {
    /// Chunks of the load region neither loaded nor being generated.
    pub missing: usize,
    pub inflight: usize,
    pub stashed: usize,
    pub cached_columns: usize,
}

#[derive(Debug)]
pub struct Streamer {
    generator: Arc<TerrainGenerator>,
    settings: StreamSettings,
    center: Option<ChunkPos>,
    /// Load region of the current centre, nearest first.
    wanted: Vec<ChunkPos>,
    /// Entries of `wanted` before this index are loaded or in flight.
    cursor: usize,
    /// Loaded chunks inside the load region (for statistics).
    loaded_in_region: usize,
    inflight: HashSet<ChunkPos>,
    tx: Sender<(ChunkPos, Chunk)>,
    rx: Receiver<(ChunkPos, Chunk)>,
    stash: HashMap<ChunkPos, Chunk>,
}

impl Streamer {
    pub fn new(generator: Arc<TerrainGenerator>, settings: StreamSettings) -> Self {
        let (tx, rx) = channel();
        Self {
            generator,
            settings,
            center: None,
            wanted: Vec::new(),
            cursor: 0,
            loaded_in_region: 0,
            inflight: HashSet::new(),
            tx,
            rx,
            stash: HashMap::new(),
        }
    }

    pub fn generator(&self) -> &Arc<TerrainGenerator> {
        &self.generator
    }

    pub fn settings(&self) -> StreamSettings {
        self.settings
    }

    pub fn stats(&self) -> StreamStats {
        StreamStats {
            missing: self
                .wanted
                .len()
                .saturating_sub(self.loaded_in_region + self.inflight.len()),
            inflight: self.inflight.len(),
            stashed: self.stash.len(),
            cached_columns: self.generator.cached_columns(),
        }
    }

    /// Whether every chunk of the load region is loaded.
    pub fn is_idle(&self) -> bool {
        self.center.is_some() && self.cursor >= self.wanted.len() && self.inflight.is_empty()
    }

    /// Advances streaming around `center`: unloads far chunks, inserts finished
    /// chunks and requests missing ones. Call once per frame.
    pub fn update(&mut self, center: ChunkPos, world: &mut World) {
        let _span = tracing::trace_span!("stream_update").entered();
        if self.center != Some(center) {
            self.recenter(center, world);
        }

        while let Ok((pos, chunk)) = self.rx.try_recv() {
            self.inflight.remove(&pos);
            if self
                .settings
                .within(center, pos, self.settings.unload_margin)
                && !world.contains(pos)
            {
                self.insert(center, pos, chunk, world);
            }
        }

        while self.inflight.len() < self.settings.max_inflight && self.cursor < self.wanted.len() {
            let pos = self.wanted[self.cursor];
            self.cursor += 1;
            if world.contains(pos) || self.inflight.contains(&pos) {
                continue;
            }
            if let Some(mut chunk) = self.stash.remove(&pos) {
                chunk.mark_all_dirty();
                self.insert(center, pos, chunk, world);
                continue;
            }
            self.inflight.insert(pos);
            let generator = Arc::clone(&self.generator);
            let tx = self.tx.clone();
            rayon::spawn(move || {
                // The receiver only disappears when the streamer is dropped.
                let _ = tx.send((pos, generator.generate(pos)));
            });
        }
    }

    fn insert(&mut self, center: ChunkPos, pos: ChunkPos, chunk: Chunk, world: &mut World) {
        if self.settings.within(center, pos, 0) {
            self.loaded_in_region += 1;
        }
        world.insert(pos, chunk);
    }

    fn recenter(&mut self, center: ChunkPos, world: &mut World) {
        let _span = tracing::trace_span!("stream_recenter").entered();
        self.center = Some(center);
        let s = self.settings;

        let far: Vec<ChunkPos> = world
            .positions()
            .filter(|&p| !s.within(center, p, s.unload_margin))
            .collect();
        for pos in far {
            if let Some(chunk) = world.remove(pos)
                && chunk.is_modified()
            {
                self.stash.insert(pos, chunk);
            }
        }
        self.loaded_in_region = world
            .positions()
            .filter(|&p| s.within(center, p, 0))
            .count();
        self.generator.retain_columns(
            glam::IVec2::new(center.0.x, center.0.z),
            (s.radius + s.unload_margin + 1) as i32,
        );

        self.wanted.clear();
        let (r, v) = (s.radius as i32, s.vertical_radius as i32);
        for z in -r..=r {
            for y in -v..=v {
                for x in -r..=r {
                    let pos = ChunkPos(center.0 + IVec3::new(x, y, z));
                    if s.within(center, pos, 0) {
                        self.wanted.push(pos);
                    }
                }
            }
        }
        self.wanted
            .sort_by_key(|p| (p.0 - center.0).length_squared());
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainPalette;
    use mv_core::config::WorldConfig;
    use mv_core::{LocalPos, WorldPos};
    use mv_materials::MaterialId;

    fn streamer(radius: u32) -> Streamer {
        let palette = TerrainPalette {
            stone: MaterialId(1),
            dirt: MaterialId(2),
            grass: MaterialId(3),
            sand: MaterialId(4),
            water: MaterialId(5),
        };
        let generator = Arc::new(TerrainGenerator::new(&WorldConfig::default(), 1, palette));
        Streamer::new(
            generator,
            StreamSettings {
                radius,
                vertical_radius: 1,
                unload_margin: 1,
                max_inflight: 8,
            },
        )
    }

    fn run_until_idle(s: &mut Streamer, center: ChunkPos, world: &mut World) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            s.update(center, world);
            if s.is_idle() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("streaming did not finish");
    }

    #[test]
    fn loads_region_and_unloads_far_chunks() {
        let mut s = streamer(2);
        let mut world = World::new();
        let origin = ChunkPos::new(0, 0, 0);
        run_until_idle(&mut s, origin, &mut world);
        // Radius 2 disc has 13 columns; 3 layers.
        assert_eq!(world.len(), 13 * 3);
        assert_eq!(s.stats().missing, 0);
        assert!(world.contains(ChunkPos::new(2, 1, 0)));
        assert!(!world.contains(ChunkPos::new(2, 0, 2)));

        let far = ChunkPos::new(10, 0, 0);
        run_until_idle(&mut s, far, &mut world);
        assert!(!world.contains(origin));
        assert!(world.contains(far));
        assert_eq!(world.len(), 13 * 3);
    }

    #[test]
    fn modified_chunks_survive_unloading() {
        let mut s = streamer(1);
        let mut world = World::new();
        let origin = ChunkPos::new(0, 0, 0);
        run_until_idle(&mut s, origin, &mut world);
        let p = WorldPos::from_chunk_local(origin, LocalPos::new(1, 2, 3));
        let marker = MaterialId(9);
        assert!(world.set_voxel(p, marker));

        run_until_idle(&mut s, ChunkPos::new(20, 0, 0), &mut world);
        assert!(!world.contains(origin));
        assert_eq!(s.stats().stashed, 1);

        run_until_idle(&mut s, origin, &mut world);
        assert_eq!(world.voxel(p), Some(marker));
        assert_eq!(s.stats().stashed, 0);
    }

    #[test]
    fn region_shape() {
        let s = StreamSettings {
            radius: 3,
            vertical_radius: 1,
            unload_margin: 0,
            max_inflight: 1,
        };
        let c = ChunkPos::new(5, 5, 5);
        assert!(s.within(c, ChunkPos::new(8, 5, 5), 0));
        assert!(!s.within(c, ChunkPos::new(8, 5, 6), 0));
        assert!(s.within(c, ChunkPos::new(5, 6, 5), 0));
        assert!(!s.within(c, ChunkPos::new(5, 7, 5), 0));
        assert!(s.within(c, ChunkPos::new(5, 7, 5), 1));
    }
}
