//! Voxel world storage.
//!
//! * [`Chunk`]: 64³ microvoxels, stored either as a single material
//!   ([`ChunkData::Uniform`]) or as a dense structure-of-arrays.
//! * [`BrickMask`]: one bit per 8³ render brick of a chunk (512 bits).
//! * [`testgen`]: deterministic test scenes used by the early milestones.
//!
//! Streaming and procedural terrain arrive in milestone 2 (see `PROMPT.md` §5).

mod brick_mask;
mod chunk;
pub mod testgen;

pub use brick_mask::BrickMask;
pub use chunk::{Chunk, ChunkData};
// Re-exported so consumers of chunk data (e.g. the renderer) need not depend on
// mv_materials directly.
pub use mv_materials::MaterialId;
