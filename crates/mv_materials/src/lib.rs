//! Material and reaction registry.
//!
//! Materials are defined in `assets/materials/*.ron` and reactions in
//! `assets/reactions/*.ron` (schemas documented in this crate's `README.md`). They are
//! loaded into contiguous tables indexed by [`MaterialId`], so hot loops can fetch
//! properties with a plain array access and reactions with an O(1) matrix lookup.

mod def;
mod reactions;
mod registry;

pub use reactions::{Reaction, ReactionSet};
pub use registry::{
    Material, MaterialId, MaterialRegistry, MatterState, RegistryError, Transition,
};
