//! Base types shared by every Chemxel crate.
//!
//! This crate must stay small and dependency-light: everything else depends on it.
//! It contains world-scale constants, coordinate types and conversions, the fixed
//! timestep clock and the runtime configuration.

pub mod config;
pub mod coords;
pub mod paths;
pub mod time;

pub use config::{Config, ConfigError};
pub use coords::{ChunkPos, LocalPos, WorldPos};
pub use time::FixedTimestep;

/// World-scale constants. See `PROMPT.md` §2.
pub mod consts {
    /// log2 of the chunk edge length in microvoxels.
    pub const CHUNK_SIZE_LOG2: u32 = 6;
    /// Chunk edge length in microvoxels (64).
    pub const CHUNK_SIZE: usize = 1 << CHUNK_SIZE_LOG2;
    /// Bit mask to extract a local coordinate from a world coordinate.
    pub const CHUNK_MASK: i64 = CHUNK_SIZE as i64 - 1;
    /// Number of microvoxels in a chunk (64³ = 262 144).
    pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

    /// log2 of the render brick edge length in microvoxels.
    pub const BRICK_SIZE_LOG2: u32 = 3;
    /// Render brick edge length in microvoxels (8).
    pub const BRICK_SIZE: usize = 1 << BRICK_SIZE_LOG2;
    /// Number of microvoxels in a brick (8³ = 512).
    pub const BRICK_VOLUME: usize = BRICK_SIZE * BRICK_SIZE * BRICK_SIZE;
    /// Bricks along one chunk axis (8).
    pub const BRICKS_PER_CHUNK_AXIS: usize = CHUNK_SIZE / BRICK_SIZE;
    /// Bricks in a chunk (8³ = 512).
    pub const BRICKS_PER_CHUNK: usize =
        BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS;

    /// Edge of a temperature cell in microvoxels (coarse thermal grid, ADR 007).
    pub const THERMAL_CELL_SIZE: usize = 4;
    /// Temperature cells along a chunk axis (16).
    pub const THERMAL_CELLS_PER_AXIS: usize = CHUNK_SIZE / THERMAL_CELL_SIZE;
    /// Temperature cells per chunk (16³ = 4096), indexed x-fastest.
    pub const THERMAL_CELLS_PER_CHUNK: usize =
        THERMAL_CELLS_PER_AXIS * THERMAL_CELLS_PER_AXIS * THERMAL_CELLS_PER_AXIS;

    /// Microvoxels along one edge of a classic 1 m block.
    pub const VOXELS_PER_BLOCK: usize = 16;
    /// Edge length of a microvoxel in metres (6.25 cm).
    pub const VOXEL_SIZE_M: f32 = 1.0 / VOXELS_PER_BLOCK as f32;
}
