//! 64³ voxel chunk.
//!
//! Only the material channel exists for now. Temperature, per-voxel flags, the
//! active/sleeping state and the dirty AABB (`PROMPT.md` §5.2) are added by the
//! milestones that need them, as extra parallel arrays (structure of arrays).

use mv_core::consts::{
    BRICK_SIZE, BRICK_SIZE_LOG2, BRICK_VOLUME, BRICKS_PER_CHUNK, BRICKS_PER_CHUNK_AXIS, CHUNK_SIZE,
    CHUNK_SIZE_LOG2, CHUNK_VOLUME,
};
use mv_core::{ChunkPos, LocalPos, WorldPos};
use mv_materials::MaterialId;

use crate::BrickMask;

/// Voxel payload of a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkData {
    /// Every voxel is the same material (all air, all stone...). No arrays allocated.
    Uniform(MaterialId),
    /// One material per voxel, indexed by [`LocalPos::index`].
    Dense(Box<[MaterialId]>),
}

/// A 64³ block of microvoxels.
#[derive(Debug, Clone)]
pub struct Chunk {
    data: ChunkData,
    /// Bricks modified since the last call to [`Chunk::take_dirty_bricks`].
    dirty_bricks: BrickMask,
}

impl Chunk {
    /// A chunk filled with a single material. All bricks start dirty.
    pub fn uniform(material: MaterialId) -> Self {
        Self {
            data: ChunkData::Uniform(material),
            dirty_bricks: BrickMask::FULL,
        }
    }

    /// Builds a chunk by evaluating `f` for every voxel, in memory order. The result
    /// is compacted to [`ChunkData::Uniform`] when possible. All bricks start dirty.
    pub fn from_fn(pos: ChunkPos, mut f: impl FnMut(WorldPos) -> MaterialId) -> Self {
        let mut voxels = vec![MaterialId::AIR; CHUNK_VOLUME].into_boxed_slice();
        for (index, voxel) in voxels.iter_mut().enumerate() {
            *voxel = f(WorldPos::from_chunk_local(pos, LocalPos::from_index(index)));
        }
        let mut chunk = Self {
            data: ChunkData::Dense(voxels),
            dirty_bricks: BrickMask::FULL,
        };
        chunk.compact();
        chunk
    }

    pub fn data(&self) -> &ChunkData {
        &self.data
    }

    /// The material if the whole chunk is a single one.
    pub fn as_uniform(&self) -> Option<MaterialId> {
        match self.data {
            ChunkData::Uniform(m) => Some(m),
            ChunkData::Dense(_) => None,
        }
    }

    #[inline]
    pub fn get(&self, pos: LocalPos) -> MaterialId {
        match &self.data {
            ChunkData::Uniform(m) => *m,
            ChunkData::Dense(voxels) => voxels[pos.index()],
        }
    }

    /// Writes one voxel, expanding a uniform chunk to dense storage if needed.
    /// Marks the containing brick dirty only if the value actually changed.
    pub fn set(&mut self, pos: LocalPos, material: MaterialId) {
        let voxels = match &mut self.data {
            ChunkData::Uniform(m) if *m == material => return,
            ChunkData::Uniform(m) => {
                let fill = *m;
                self.data = ChunkData::Dense(vec![fill; CHUNK_VOLUME].into_boxed_slice());
                let ChunkData::Dense(voxels) = &mut self.data else {
                    unreachable!("just assigned Dense");
                };
                voxels
            }
            ChunkData::Dense(voxels) => voxels,
        };
        let slot = &mut voxels[pos.index()];
        if *slot != material {
            *slot = material;
            self.dirty_bricks.set(pos.brick_index());
        }
    }

    /// Converts dense storage back to uniform if every voxel is equal. Returns
    /// `true` if the chunk is uniform afterwards.
    pub fn compact(&mut self) -> bool {
        if let ChunkData::Dense(voxels) = &self.data {
            let first = voxels[0];
            if voxels.iter().all(|&v| v == first) {
                self.data = ChunkData::Uniform(first);
            }
        }
        self.as_uniform().is_some()
    }

    /// Returns the set of bricks modified since the previous call, and resets it.
    pub fn take_dirty_bricks(&mut self) -> BrickMask {
        self.dirty_bricks.take()
    }

    pub fn dirty_bricks(&self) -> BrickMask {
        self.dirty_bricks
    }

    /// Copies brick `brick` into `out` in brick-local order (`x + y*8 + z*64`).
    /// Returns `true` if the brick contains any non-air voxel.
    pub fn copy_brick(&self, brick: usize, out: &mut [MaterialId; BRICK_VOLUME]) -> bool {
        debug_assert!(brick < BRICKS_PER_CHUNK);
        match &self.data {
            ChunkData::Uniform(m) => {
                out.fill(*m);
                !m.is_air()
            }
            ChunkData::Dense(voxels) => {
                let origin = brick_origin(brick);
                let mut occupied = false;
                for z in 0..BRICK_SIZE {
                    for y in 0..BRICK_SIZE {
                        let src = (origin[0])
                            | ((origin[1] + y) << CHUNK_SIZE_LOG2)
                            | ((origin[2] + z) << (2 * CHUNK_SIZE_LOG2));
                        let dst = (y << BRICK_SIZE_LOG2) | (z << (2 * BRICK_SIZE_LOG2));
                        let row = &voxels[src..src + BRICK_SIZE];
                        out[dst..dst + BRICK_SIZE].copy_from_slice(row);
                        occupied |= row.iter().any(|v| !v.is_air());
                    }
                }
                occupied
            }
        }
    }

    /// Bitmask of bricks that contain at least one non-air voxel.
    pub fn occupancy(&self) -> BrickMask {
        match &self.data {
            ChunkData::Uniform(m) if m.is_air() => BrickMask::EMPTY,
            ChunkData::Uniform(_) => BrickMask::FULL,
            ChunkData::Dense(voxels) => {
                let mut mask = BrickMask::EMPTY;
                // Walk memory in order; mark the brick of each non-air row segment.
                for (row_index, row) in voxels.as_chunks::<BRICK_SIZE>().0.iter().enumerate() {
                    if row.iter().any(|v| !v.is_air()) {
                        let first = LocalPos::from_index(row_index * BRICK_SIZE);
                        mask.set(first.brick_index());
                    }
                }
                mask
            }
        }
    }
}

/// Local voxel coordinates of the minimum corner of `brick`.
#[inline]
fn brick_origin(brick: usize) -> [usize; 3] {
    let a = BRICKS_PER_CHUNK_AXIS;
    [
        (brick % a) * BRICK_SIZE,
        ((brick / a) % a) * BRICK_SIZE,
        (brick / (a * a)) * BRICK_SIZE,
    ]
}

const _: () = assert!(CHUNK_SIZE.is_multiple_of(BRICK_SIZE));

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: MaterialId = MaterialId(1);
    const SAND: MaterialId = MaterialId(2);

    #[test]
    fn uniform_get_and_noop_set() {
        let mut c = Chunk::uniform(STONE);
        c.take_dirty_bricks();
        c.set(LocalPos::new(5, 5, 5), STONE);
        assert_eq!(c.as_uniform(), Some(STONE));
        assert!(c.dirty_bricks().is_empty());
    }

    #[test]
    fn set_densifies_and_marks_only_its_brick() {
        let mut c = Chunk::uniform(MaterialId::AIR);
        c.take_dirty_bricks();
        let p = LocalPos::new(9, 17, 33);
        c.set(p, SAND);
        assert_eq!(c.as_uniform(), None);
        assert_eq!(c.get(p), SAND);
        assert_eq!(c.get(LocalPos::new(0, 0, 0)), MaterialId::AIR);
        let dirty = c.take_dirty_bricks();
        assert_eq!(dirty.iter().collect::<Vec<_>>(), vec![p.brick_index()]);
    }

    #[test]
    fn compact_restores_uniform() {
        let mut c = Chunk::uniform(MaterialId::AIR);
        let p = LocalPos::new(1, 2, 3);
        c.set(p, SAND);
        assert!(!c.compact());
        c.set(p, MaterialId::AIR);
        assert!(c.compact());
        assert_eq!(c.as_uniform(), Some(MaterialId::AIR));
    }

    #[test]
    fn from_fn_uses_world_coordinates() {
        let pos = ChunkPos::new(-1, 0, 0);
        let c = Chunk::from_fn(pos, |w| if w.0.x == -1 { SAND } else { MaterialId::AIR });
        assert_eq!(c.get(LocalPos::new(63, 0, 0)), SAND);
        assert_eq!(c.get(LocalPos::new(62, 0, 0)), MaterialId::AIR);
    }

    #[test]
    fn copy_brick_layout_and_occupancy() {
        let mut c = Chunk::uniform(MaterialId::AIR);
        // Voxel (10, 3, 17) lives in brick (1, 0, 2) at brick-local (2, 3, 1).
        let p = LocalPos::new(10, 3, 17);
        c.set(p, SAND);
        let brick = p.brick_index();
        assert_eq!(brick, 1 + 2 * 64);

        let mut out = [MaterialId::AIR; BRICK_VOLUME];
        assert!(c.copy_brick(brick, &mut out));
        let local = 2 + 3 * 8 + 64;
        assert_eq!(out[local], SAND);
        assert_eq!(out.iter().filter(|v| !v.is_air()).count(), 1);

        assert!(!c.copy_brick(0, &mut out));
        assert_eq!(c.occupancy().iter().collect::<Vec<_>>(), vec![brick]);
    }

    #[test]
    fn uniform_occupancy() {
        assert!(Chunk::uniform(MaterialId::AIR).occupancy().is_empty());
        assert_eq!(Chunk::uniform(STONE).occupancy(), BrickMask::FULL);
    }
}
