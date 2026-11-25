//! The set of loaded chunks, plus change queues for consumers such as the renderer.
//!
//! Lookup is one `HashMap` access per *chunk*; per-voxel loops must fetch the chunk
//! once and work on it (see [`crate::edit`]).

use std::collections::{HashMap, VecDeque};

use mv_core::{ChunkPos, WorldPos};
use mv_materials::MaterialId;

use crate::Chunk;

/// Memory statistics of the loaded chunks.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorldStats {
    pub chunks: usize,
    pub uniform_chunks: usize,
    pub dense_bricks: usize,
    pub heap_bytes: usize,
}

#[derive(Debug, Default)]
pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
    /// Chunks inserted since they were last popped (streamed in).
    loaded: VecDeque<ChunkPos>,
    /// Chunks edited since the last [`World::take_edited`] (no duplicates).
    edited: Vec<ChunkPos>,
    /// Chunks removed since the last [`World::take_unloaded`].
    unloaded: Vec<ChunkPos>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn contains(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// Mutable access without change tracking. Use [`World::mark_edited`] after
    /// changing voxels, or [`World::set_voxel`].
    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    pub fn iter(&self) -> impl Iterator<Item = (ChunkPos, &Chunk)> {
        self.chunks.iter().map(|(p, c)| (*p, c))
    }

    pub fn positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    /// Inserts (or replaces) a chunk and queues it as loaded.
    pub fn insert(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.chunks.insert(pos, chunk);
        self.loaded.push_back(pos);
    }

    /// Removes a chunk and queues it as unloaded.
    pub fn remove(&mut self, pos: ChunkPos) -> Option<Chunk> {
        let chunk = self.chunks.remove(&pos)?;
        self.unloaded.push(pos);
        Some(chunk)
    }

    /// Temporarily takes a chunk out without queueing it as unloaded, so a system can
    /// mutate it while sharing the rest of the world read-only (e.g. the simulation's
    /// parallel phases). Must be returned with [`World::attach`].
    pub fn detach(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }

    /// Puts back a chunk taken with [`World::detach`] (no queue side effects).
    pub fn attach(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.chunks.insert(pos, chunk);
    }

    /// Material at `pos`, or `None` if its chunk is not loaded.
    pub fn voxel(&self, pos: WorldPos) -> Option<MaterialId> {
        let (c, l) = pos.to_chunk_local();
        self.chunks.get(&c).map(|chunk| chunk.get(l))
    }

    /// Fill level of the voxel at `pos` (1..=`LEVELS`), if loaded.
    pub fn level(&self, pos: WorldPos) -> Option<u8> {
        let (c, l) = pos.to_chunk_local();
        self.chunks.get(&c).map(|chunk| chunk.level(l))
    }

    /// Writes one voxel with a fill level (see [`Chunk::set_with_level`]).
    pub fn set_voxel_level(&mut self, pos: WorldPos, material: MaterialId, level: u8) -> bool {
        let (c, l) = pos.to_chunk_local();
        let changed = self
            .chunks
            .get_mut(&c)
            .is_some_and(|chunk| chunk.set_with_level(l, material, level));
        if changed {
            self.mark_edited(c);
        }
        changed
    }

    /// Writes one voxel. Returns whether it changed (false if not loaded).
    pub fn set_voxel(&mut self, pos: WorldPos, material: MaterialId) -> bool {
        let (c, l) = pos.to_chunk_local();
        let changed = self
            .chunks
            .get_mut(&c)
            .is_some_and(|chunk| chunk.set(l, material));
        if changed {
            self.mark_edited(c);
        }
        changed
    }

    /// Queues `pos` as edited (once until the next [`World::take_edited`]).
    pub fn mark_edited(&mut self, pos: ChunkPos) {
        // Edits touch a handful of chunks per frame; a linear scan is cheapest.
        if !self.edited.contains(&pos) {
            self.edited.push(pos);
        }
    }

    pub fn take_edited(&mut self) -> Vec<ChunkPos> {
        std::mem::take(&mut self.edited)
    }

    pub fn take_unloaded(&mut self) -> Vec<ChunkPos> {
        std::mem::take(&mut self.unloaded)
    }

    /// Next streamed-in chunk to process. May yield positions that were unloaded
    /// again since; callers must look the chunk up and skip missing ones.
    pub fn pop_loaded(&mut self) -> Option<ChunkPos> {
        self.loaded.pop_front()
    }

    /// Puts a position back at the front of the loaded queue (e.g. budget exhausted).
    pub fn unpop_loaded(&mut self, pos: ChunkPos) {
        self.loaded.push_front(pos);
    }

    pub fn pending_loaded(&self) -> usize {
        self.loaded.len()
    }

    /// Walks every chunk; meant for occasional debug statistics.
    pub fn stats(&self) -> WorldStats {
        let mut s = WorldStats {
            chunks: self.chunks.len(),
            ..WorldStats::default()
        };
        for chunk in self.chunks.values() {
            if chunk.as_uniform().is_some() {
                s.uniform_chunks += 1;
            }
            s.dense_bricks += chunk.dense_bricks();
            s.heap_bytes += chunk.heap_bytes();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAND: MaterialId = MaterialId(2);

    #[test]
    fn voxel_access_across_chunks() {
        let mut w = World::new();
        w.insert(ChunkPos::new(-1, 0, 0), Chunk::uniform(MaterialId::AIR));
        w.insert(ChunkPos::new(0, 0, 0), Chunk::uniform(MaterialId::AIR));
        assert!(w.set_voxel(WorldPos::new(-1, 5, 5), SAND));
        assert!(!w.set_voxel(WorldPos::new(-1, 5, 5), SAND));
        assert_eq!(w.voxel(WorldPos::new(-1, 5, 5)), Some(SAND));
        assert_eq!(w.voxel(WorldPos::new(0, 5, 5)), Some(MaterialId::AIR));
        assert_eq!(w.voxel(WorldPos::new(64, 5, 5)), None);
        assert!(!w.set_voxel(WorldPos::new(64, 5, 5), SAND));
        assert_eq!(w.take_edited(), vec![ChunkPos::new(-1, 0, 0)]);
        assert!(w.take_edited().is_empty());
    }

    #[test]
    fn queues() {
        let mut w = World::new();
        let a = ChunkPos::new(0, 0, 0);
        let b = ChunkPos::new(1, 0, 0);
        w.insert(a, Chunk::uniform(MaterialId::AIR));
        w.insert(b, Chunk::uniform(MaterialId::AIR));
        assert_eq!(w.pending_loaded(), 2);
        assert_eq!(w.pop_loaded(), Some(a));
        w.unpop_loaded(a);
        assert_eq!(w.pop_loaded(), Some(a));
        assert!(w.remove(b).is_some());
        assert!(w.remove(b).is_none());
        assert_eq!(w.take_unloaded(), vec![b]);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn detach_and_attach_are_silent() {
        let mut w = World::new();
        let a = ChunkPos::new(0, 0, 0);
        w.insert(a, Chunk::uniform(MaterialId::AIR));
        w.pop_loaded();
        let c = w.detach(a).unwrap();
        assert!(!w.contains(a));
        w.attach(a, c);
        assert!(w.contains(a));
        assert!(w.take_unloaded().is_empty());
        assert_eq!(w.pending_loaded(), 0);
    }

    #[test]
    fn stats_count_dense_bricks() {
        let mut w = World::new();
        w.insert(ChunkPos::new(0, 0, 0), Chunk::uniform(MaterialId::AIR));
        w.set_voxel(WorldPos::new(1, 1, 1), SAND);
        w.set_voxel(WorldPos::new(60, 1, 1), SAND);
        let s = w.stats();
        assert_eq!(s.chunks, 1);
        assert_eq!(s.uniform_chunks, 0);
        assert_eq!(s.dense_bricks, 2);
    }
}
