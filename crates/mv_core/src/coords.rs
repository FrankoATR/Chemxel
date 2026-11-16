//! Coordinate types and conversions.
//!
//! Three coordinate spaces exist:
//!
//! * [`WorldPos`]: absolute position in microvoxels (`i64` per axis).
//! * [`ChunkPos`]: position of a chunk in chunk units (`i32` per axis).
//! * [`LocalPos`]: position of a microvoxel inside its chunk (`0..64` per axis).
//!
//! Conversions use arithmetic shifts and masks (chunk size is 2⁶), which handle
//! negative coordinates correctly: world `x = -1` lives in chunk `-1` at local `63`.
//!
//! # Linear index order
//!
//! Voxel data inside a chunk is stored in a flat array indexed by
//! [`LocalPos::index`] = `x + y * 64 + z * 64²` (X fastest, then Y, then Z).
//! Every crate must use this order; iterate `z` outermost and `x` innermost to walk
//! memory sequentially.

use glam::{I64Vec3, IVec3};
use serde::{Deserialize, Serialize};

use crate::consts::{
    BRICK_SIZE_LOG2, BRICKS_PER_CHUNK_AXIS, CHUNK_MASK, CHUNK_SIZE, CHUNK_SIZE_LOG2, CHUNK_VOLUME,
};

/// Absolute microvoxel position in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct WorldPos(pub I64Vec3);

/// Chunk coordinates (one unit = one 64³ chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct ChunkPos(pub IVec3);

/// Microvoxel position inside a chunk. Every component is in `0..64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct LocalPos {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl WorldPos {
    #[inline]
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self(I64Vec3::new(x, y, z))
    }

    /// Builds a world position from a chunk and a local position inside it.
    #[inline]
    pub fn from_chunk_local(chunk: ChunkPos, local: LocalPos) -> Self {
        let origin = chunk.origin().0;
        Self(origin + I64Vec3::new(local.x.into(), local.y.into(), local.z.into()))
    }

    /// Chunk containing this position.
    #[inline]
    pub fn chunk(self) -> ChunkPos {
        let c = self.0 >> CHUNK_SIZE_LOG2 as i64;
        // A world coordinate of ±2⁶³ maps to chunk ±2⁵⁷, which does not fit in i32.
        // Worlds that large are out of scope; truncation is documented and accepted.
        ChunkPos(IVec3::new(c.x as i32, c.y as i32, c.z as i32))
    }

    /// Position inside the containing chunk.
    #[inline]
    pub fn local(self) -> LocalPos {
        let l = self.0 & I64Vec3::splat(CHUNK_MASK);
        LocalPos {
            x: l.x as u8,
            y: l.y as u8,
            z: l.z as u8,
        }
    }

    /// Splits this position into its chunk and local parts.
    #[inline]
    pub fn to_chunk_local(self) -> (ChunkPos, LocalPos) {
        (self.chunk(), self.local())
    }
}

impl ChunkPos {
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self(IVec3::new(x, y, z))
    }

    /// World position of the chunk's `(0, 0, 0)` voxel.
    #[inline]
    pub fn origin(self) -> WorldPos {
        WorldPos(self.0.as_i64vec3() << CHUNK_SIZE_LOG2 as i64)
    }
}

impl LocalPos {
    /// Creates a local position.
    ///
    /// # Panics
    /// In debug builds, if any component is `>= 64`.
    #[inline]
    pub fn new(x: u8, y: u8, z: u8) -> Self {
        debug_assert!(
            (x as usize) < CHUNK_SIZE && (y as usize) < CHUNK_SIZE && (z as usize) < CHUNK_SIZE,
            "LocalPos out of range: ({x}, {y}, {z})"
        );
        Self { x, y, z }
    }

    /// Linear index into a chunk array: `x + y * 64 + z * 64²`.
    #[inline]
    pub const fn index(self) -> usize {
        (self.x as usize)
            | ((self.y as usize) << CHUNK_SIZE_LOG2)
            | ((self.z as usize) << (2 * CHUNK_SIZE_LOG2))
    }

    /// Inverse of [`LocalPos::index`].
    ///
    /// # Panics
    /// In debug builds, if `index >= CHUNK_VOLUME`.
    #[inline]
    pub const fn from_index(index: usize) -> Self {
        debug_assert!(index < CHUNK_VOLUME);
        let mask = CHUNK_SIZE - 1;
        Self {
            x: (index & mask) as u8,
            y: ((index >> CHUNK_SIZE_LOG2) & mask) as u8,
            z: ((index >> (2 * CHUNK_SIZE_LOG2)) & mask) as u8,
        }
    }

    /// Linear index of the 8³ render brick containing this voxel, using the same
    /// X-fastest order over the 8×8×8 brick grid of a chunk.
    #[inline]
    pub const fn brick_index(self) -> usize {
        let bx = (self.x >> BRICK_SIZE_LOG2) as usize;
        let by = (self.y >> BRICK_SIZE_LOG2) as usize;
        let bz = (self.z >> BRICK_SIZE_LOG2) as usize;
        bx + by * BRICKS_PER_CHUNK_AXIS + bz * BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS
    }

    #[inline]
    pub fn as_ivec3(self) -> IVec3 {
        IVec3::new(self.x.into(), self.y.into(), self.z.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::BRICKS_PER_CHUNK;

    fn split(x: i64) -> (i32, u8) {
        let (c, l) = WorldPos::new(x, 0, 0).to_chunk_local();
        (c.0.x, l.x)
    }

    #[test]
    fn chunk_boundaries_positive() {
        assert_eq!(split(0), (0, 0));
        assert_eq!(split(63), (0, 63));
        assert_eq!(split(64), (1, 0));
        assert_eq!(split(127), (1, 63));
        assert_eq!(split(128), (2, 0));
    }

    #[test]
    fn chunk_boundaries_negative() {
        assert_eq!(split(-1), (-1, 63));
        assert_eq!(split(-64), (-1, 0));
        assert_eq!(split(-65), (-2, 63));
        assert_eq!(split(-128), (-2, 0));
        assert_eq!(split(-129), (-3, 63));
    }

    #[test]
    fn world_chunk_local_roundtrip() {
        for x in (-300..300).step_by(7) {
            for y in (-300..300).step_by(11) {
                for z in [-200, -65, -64, -63, -1, 0, 1, 63, 64, 65, 200] {
                    let w = WorldPos::new(x, y, z);
                    let (c, l) = w.to_chunk_local();
                    assert_eq!(
                        WorldPos::from_chunk_local(c, l),
                        w,
                        "roundtrip failed for {w:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn axes_are_independent() {
        let (c, l) = WorldPos::new(-1, 64, 130).to_chunk_local();
        assert_eq!(c, ChunkPos::new(-1, 1, 2));
        assert_eq!(l, LocalPos::new(63, 0, 2));
    }

    #[test]
    fn large_coordinates() {
        let big = i64::from(i32::MAX) * 64 + 5;
        let small = i64::from(i32::MIN) * 64 + 59;
        let (c, l) = WorldPos::new(big, small, 0).to_chunk_local();
        assert_eq!(c.0.x, i32::MAX);
        assert_eq!(l.x, 5);
        assert_eq!(c.0.y, i32::MIN);
        assert_eq!(l.y, 59);
    }

    #[test]
    fn chunk_origin() {
        assert_eq!(ChunkPos::new(0, 0, 0).origin(), WorldPos::new(0, 0, 0));
        assert_eq!(
            ChunkPos::new(1, -1, 2).origin(),
            WorldPos::new(64, -64, 128)
        );
    }

    #[test]
    fn local_index_roundtrip_exhaustive() {
        for i in 0..CHUNK_VOLUME {
            assert_eq!(LocalPos::from_index(i).index(), i);
        }
    }

    #[test]
    fn local_index_order_is_x_fastest() {
        assert_eq!(LocalPos::new(1, 0, 0).index(), 1);
        assert_eq!(LocalPos::new(0, 1, 0).index(), 64);
        assert_eq!(LocalPos::new(0, 0, 1).index(), 64 * 64);
        assert_eq!(LocalPos::new(63, 63, 63).index(), CHUNK_VOLUME - 1);
    }

    #[test]
    fn brick_index_covers_all_bricks() {
        assert_eq!(LocalPos::new(0, 0, 0).brick_index(), 0);
        assert_eq!(LocalPos::new(7, 7, 7).brick_index(), 0);
        assert_eq!(LocalPos::new(8, 0, 0).brick_index(), 1);
        assert_eq!(LocalPos::new(0, 8, 0).brick_index(), 8);
        assert_eq!(LocalPos::new(0, 0, 8).brick_index(), 64);
        assert_eq!(
            LocalPos::new(63, 63, 63).brick_index(),
            BRICKS_PER_CHUNK - 1
        );
    }
}
