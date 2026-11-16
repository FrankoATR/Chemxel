//! Material registry.
//!
//! Materials are defined in `assets/materials/*.ron` (schema documented in this
//! crate's `README.md`) and loaded at startup into a contiguous table indexed by
//! [`MaterialId`], so hot loops can fetch properties with a plain array access.
//!
//! Chemical reactions (`assets/reactions/*.ron`) are added in milestone 5.

mod def;
mod registry;

pub use registry::{
    Material, MaterialId, MaterialRegistry, MatterState, RegistryError, Transition,
};
