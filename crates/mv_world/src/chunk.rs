//! 64³ voxel chunk with brick-sparse storage (ADR 004).
//!
//! A chunk is either a single material ([`ChunkData::Uniform`]) or a table of 512
//! bricks of 8³ voxels. Each [`Brick`] is again either uniform (no allocation) or
//! dense (512 materials, 1 KiB). Terrain is mostly uniform away from surfaces, so
//! memory grows with surface area instead of volume.
//!
//! Inside a dense brick, voxels are indexed by [`LocalPos::brick_local_index`]
//! (`x + y*8 + z*64`); bricks by [`LocalPos::brick_index`]. Iterate bricks in index
//! order and voxels `z`, `y`, `x` (outer to inner) to walk memory sequentially.
//!
//! Only the material channel exists for now. Temperature, per-voxel flags, the
//! active/sleeping state and the dirty AABB (`PROMPT.md` §5.2) are added by the
//! milestones that need them.

use mv_core::consts::{BRICK_VOLUME, BRICKS_PER_CHUNK};
use mv_core::{ChunkPos, LocalPos, WorldPos};
use mv_materials::MaterialId;

use crate::BrickMask;

/// Voxels of one dense brick, in brick-local order.
pub type BrickVoxels = [MaterialId; BRICK_VOLUME];

/// One 8³ brick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Brick {
    /// Every voxel of the brick is this material.
    Uniform(MaterialId),
    /// One material per voxel, indexed by [`LocalPos::brick_local_index`].
    Dense(Box<BrickVoxels>),
}

impl Brick {
    /// Builds a brick from its voxels, compacting to [`Brick::Uniform`] if possible.
    pub fn from_voxels(voxels: &BrickVoxels) -> Self {
        let first = voxels[0];
        if voxels.iter().all(|&v| v == first) {
            Self::Uniform(first)
        } else {
            Self::Dense(Box::new(*voxels))
        }
    }

    #[inline]
    pub fn get(&self, local: usize) -> MaterialId {
        match self {
            Self::Uniform(m) => *m,
            Self::Dense(v) => v[local],
        }
    }

    /// The material if every voxel is the same, scanning dense bricks too.
    pub fn uniform_material(&self) -> Option<MaterialId> {
        match self {
            Self::Uniform(m) => Some(*m),
            Self::Dense(v) => {
                let first = v[0];
                v.iter().all(|&m| m == first).then_some(first)
            }
        }
    }

    /// Converts a dense brick whose voxels are all equal to uniform.
    fn compact(&mut self) {
        if let Self::Dense(_) = self
            && let Some(m) = self.uniform_material()
        {
            *self = Self::Uniform(m);
        }
    }
}

/// Borrowed view of a brick, valid for both chunk representations.
#[derive(Debug, Clone, Copy)]
pub enum BrickRef<'a> {
    Uniform(MaterialId),
    Dense(&'a BrickVoxels),
}

impl BrickRef<'_> {
    /// The material if every voxel is the same, scanning dense bricks too.
    pub fn uniform_material(self) -> Option<MaterialId> {
        match self {
            Self::Uniform(m) => Some(m),
            Self::Dense(v) => {
                let first = v[0];
                v.iter().all(|&m| m == first).then_some(first)
            }
        }
    }
}

/// Table of the 512 bricks of a chunk.
pub type BrickTable = [Brick; BRICKS_PER_CHUNK];

/// Voxel payload of a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkData {
    /// Every voxel is the same material (all air, all stone...). No allocation.
    Uniform(MaterialId),
    /// Per-brick storage, indexed by [`LocalPos::brick_index`].
    Bricks(Box<BrickTable>),
}

/// A 64³ block of microvoxels.
#[derive(Debug, Clone)]
pub struct Chunk {
    data: ChunkData,
    /// Bricks modified since the last call to [`Chunk::take_dirty_bricks`].
    dirty_bricks: BrickMask,
    /// Whether any voxel changed since the chunk was generated. Modified chunks must
    /// be kept (and later saved) instead of regenerated.
    modified: bool,
}

fn uniform_table(material: MaterialId) -> Box<BrickTable> {
    Box::new(std::array::from_fn(|_| Brick::Uniform(material)))
}

impl Chunk {
    /// A chunk filled with a single material. All bricks start dirty.
    pub fn uniform(material: MaterialId) -> Self {
        Self {
            data: ChunkData::Uniform(material),
            dirty_bricks: BrickMask::FULL,
            modified: false,
        }
    }

    /// Builds a chunk from a brick table, compacting it. All bricks start dirty.
    pub fn from_bricks(bricks: Box<BrickTable>) -> Self {
        let mut chunk = Self {
            data: ChunkData::Bricks(bricks),
            dirty_bricks: BrickMask::FULL,
            modified: false,
        };
        chunk.compact();
        chunk
    }

    /// Builds a chunk by evaluating `f` for every voxel, brick by brick. The result is
    /// compacted. All bricks start dirty. Meant for tests and simple generators.
    pub fn from_fn(pos: ChunkPos, mut f: impl FnMut(WorldPos) -> MaterialId) -> Self {
        let mut scratch = [MaterialId::AIR; BRICK_VOLUME];
        let bricks = Box::new(std::array::from_fn(|brick| {
            for (local, voxel) in scratch.iter_mut().enumerate() {
                *voxel = f(WorldPos::from_chunk_local(
                    pos,
                    LocalPos::from_brick(brick, local),
                ));
            }
            Brick::from_voxels(&scratch)
        }));
        Self::from_bricks(bricks)
    }

    pub fn data(&self) -> &ChunkData {
        &self.data
    }

    /// The material if the whole chunk is a single one.
    pub fn as_uniform(&self) -> Option<MaterialId> {
        match self.data {
            ChunkData::Uniform(m) => Some(m),
            ChunkData::Bricks(_) => None,
        }
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    #[inline]
    pub fn get(&self, pos: LocalPos) -> MaterialId {
        match &self.data {
            ChunkData::Uniform(m) => *m,
            ChunkData::Bricks(b) => b[pos.brick_index()].get(pos.brick_local_index()),
        }
    }

    /// Writes one voxel, expanding uniform storage as needed. Marks the containing
    /// brick dirty and the chunk modified only if the value actually changed.
    /// Returns whether it changed.
    pub fn set(&mut self, pos: LocalPos, material: MaterialId) -> bool {
        let bricks = match &mut self.data {
            ChunkData::Uniform(m) if *m == material => return false,
            ChunkData::Uniform(m) => {
                let fill = *m;
                self.data = ChunkData::Bricks(uniform_table(fill));
                let ChunkData::Bricks(b) = &mut self.data else {
                    unreachable!("just assigned Bricks");
                };
                b
            }
            ChunkData::Bricks(b) => b,
        };
        let brick_index = pos.brick_index();
        let local = pos.brick_local_index();
        let brick = &mut bricks[brick_index];
        match brick {
            Brick::Uniform(m) if *m == material => return false,
            Brick::Uniform(m) => {
                let mut voxels = Box::new([*m; BRICK_VOLUME]);
                voxels[local] = material;
                *brick = Brick::Dense(voxels);
            }
            Brick::Dense(voxels) => {
                if voxels[local] == material {
                    return false;
                }
                voxels[local] = material;
            }
        }
        self.dirty_bricks.set(brick_index);
        self.modified = true;
        true
    }

    /// Borrowed view of brick `index`.
    #[inline]
    pub fn brick(&self, index: usize) -> BrickRef<'_> {
        debug_assert!(index < BRICKS_PER_CHUNK);
        match &self.data {
            ChunkData::Uniform(m) => BrickRef::Uniform(*m),
            ChunkData::Bricks(b) => match &b[index] {
                Brick::Uniform(m) => BrickRef::Uniform(*m),
                Brick::Dense(v) => BrickRef::Dense(v),
            },
        }
    }

    /// Converts dense bricks whose voxels are all equal to uniform, then the whole
    /// chunk if all bricks are the same material. Returns `true` if the chunk is
    /// uniform afterwards. Does not touch the dirty mask.
    pub fn compact(&mut self) -> bool {
        if let ChunkData::Bricks(bricks) = &mut self.data {
            for brick in bricks.iter_mut() {
                brick.compact();
            }
            if let Brick::Uniform(first) = bricks[0]
                && bricks.iter().all(|b| *b == Brick::Uniform(first))
            {
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

    /// Marks every brick dirty (e.g. after the GPU copy of the chunk was dropped).
    pub fn mark_all_dirty(&mut self) {
        self.dirty_bricks = BrickMask::FULL;
    }

    /// Bitmask of bricks that contain at least one non-air voxel.
    pub fn occupancy(&self) -> BrickMask {
        match &self.data {
            ChunkData::Uniform(m) if m.is_air() => BrickMask::EMPTY,
            ChunkData::Uniform(_) => BrickMask::FULL,
            ChunkData::Bricks(bricks) => {
                let mut mask = BrickMask::EMPTY;
                for (i, brick) in bricks.iter().enumerate() {
                    let occupied = match brick {
                        Brick::Uniform(m) => !m.is_air(),
                        Brick::Dense(v) => v.iter().any(|m| !m.is_air()),
                    };
                    if occupied {
                        mask.set(i);
                    }
                }
                mask
            }
        }
    }

    /// Number of dense (allocated) bricks.
    pub fn dense_bricks(&self) -> usize {
        match &self.data {
            ChunkData::Uniform(_) => 0,
            ChunkData::Bricks(b) => b.iter().filter(|b| matches!(b, Brick::Dense(_))).count(),
        }
    }

    /// Approximate heap memory used by the voxel data, in bytes.
    pub fn heap_bytes(&self) -> usize {
        match &self.data {
            ChunkData::Uniform(_) => 0,
            ChunkData::Bricks(_) => {
                std::mem::size_of::<BrickTable>()
                    + self.dense_bricks() * std::mem::size_of::<BrickVoxels>()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: MaterialId = MaterialId(1);
    const SAND: MaterialId = MaterialId(2);

    #[test]
    fn uniform_get_and_noop_set() {
        let mut c = Chunk::uniform(STONE);
        c.take_dirty_bricks();
        assert!(!c.set(LocalPos::new(5, 5, 5), STONE));
        assert_eq!(c.as_uniform(), Some(STONE));
        assert!(c.dirty_bricks().is_empty());
        assert!(!c.is_modified());
    }

    #[test]
    fn set_allocates_one_brick_and_marks_it() {
        let mut c = Chunk::uniform(MaterialId::AIR);
        c.take_dirty_bricks();
        let p = LocalPos::new(9, 17, 33);
        assert!(c.set(p, SAND));
        assert!(c.is_modified());
        assert_eq!(c.as_uniform(), None);
        assert_eq!(c.dense_bricks(), 1);
        assert_eq!(c.get(p), SAND);
        assert_eq!(c.get(LocalPos::new(8, 16, 32)), MaterialId::AIR);
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
    fn compact_keeps_mixed_uniform_bricks() {
        // Half stone, half air, split on a brick boundary: no dense bricks at all.
        let c = Chunk::from_fn(ChunkPos::new(0, 0, 0), |w| {
            if w.0.y < 32 { STONE } else { MaterialId::AIR }
        });
        assert_eq!(c.as_uniform(), None);
        assert_eq!(c.dense_bricks(), 0);
        assert_eq!(c.get(LocalPos::new(3, 31, 60)), STONE);
        assert_eq!(c.get(LocalPos::new(3, 32, 60)), MaterialId::AIR);
    }

    #[test]
    fn from_fn_uses_world_coordinates() {
        let pos = ChunkPos::new(-1, 0, 0);
        let c = Chunk::from_fn(pos, |w| if w.0.x == -1 { SAND } else { MaterialId::AIR });
        assert_eq!(c.get(LocalPos::new(63, 0, 0)), SAND);
        assert_eq!(c.get(LocalPos::new(62, 0, 0)), MaterialId::AIR);
        assert_eq!(c.dense_bricks(), 64);
    }

    #[test]
    fn brick_view_and_occupancy() {
        let mut c = Chunk::uniform(MaterialId::AIR);
        // Voxel (10, 3, 17) lives in brick (1, 0, 2) at brick-local (2, 3, 1).
        let p = LocalPos::new(10, 3, 17);
        c.set(p, SAND);
        let brick = p.brick_index();
        assert_eq!(brick, 1 + 2 * 64);

        let BrickRef::Dense(voxels) = c.brick(brick) else {
            panic!("edited brick must be dense");
        };
        assert_eq!(voxels[2 + 3 * 8 + 64], SAND);
        assert_eq!(voxels.iter().filter(|v| !v.is_air()).count(), 1);
        assert_eq!(c.brick(0).uniform_material(), Some(MaterialId::AIR));
        assert_eq!(c.occupancy().iter().collect::<Vec<_>>(), vec![brick]);
    }

    #[test]
    fn uniform_occupancy() {
        assert!(Chunk::uniform(MaterialId::AIR).occupancy().is_empty());
        assert_eq!(Chunk::uniform(STONE).occupancy(), BrickMask::FULL);
    }

    #[test]
    fn heap_bytes_grow_per_dense_brick() {
        let mut c = Chunk::uniform(STONE);
        assert_eq!(c.heap_bytes(), 0);
        c.set(LocalPos::new(0, 0, 0), SAND);
        let one = c.heap_bytes();
        c.set(LocalPos::new(63, 63, 63), SAND);
        assert_eq!(c.heap_bytes() - one, BRICK_VOLUME * 2);
    }
}
