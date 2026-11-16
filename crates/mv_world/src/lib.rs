//! Voxel world storage, procedural generation and streaming.
//!
//! * [`Chunk`]: 64³ microvoxels, stored as a single material or as 512 bricks of 8³
//!   that are themselves uniform or dense (ADR 004).
//! * [`BrickMask`]: one bit per 8³ render brick of a chunk (512 bits).
//! * [`World`]: the loaded chunks plus change queues (loaded / edited / unloaded).
//! * [`terrain`]: deterministic heightmap terrain from noise.
//! * [`stream`]: loads chunks around a centre in the background, unloads far ones.
//! * [`raycast`]: voxel picking with a CPU DDA.
//! * [`edit`]: editing tools (spherical brush).
//! * [`testgen`]: small deterministic scenes for tests and benchmarks.
//!
//! This crate is headless: no rendering or windowing dependencies.

mod brick_mask;
mod chunk;
pub mod edit;
pub mod raycast;
pub mod stream;
pub mod terrain;
pub mod testgen;
mod world;

pub use brick_mask::BrickMask;
pub use chunk::{Brick, BrickRef, BrickTable, BrickVoxels, Chunk, ChunkData, material_bit};
// Re-exported so consumers of chunk data (e.g. the renderer) need not depend on
// mv_materials directly.
pub use mv_materials::MaterialId;
pub use world::{World, WorldStats};
