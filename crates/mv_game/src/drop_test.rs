//! Test scenarios spawned in the live game (overlay buttons and `--bench-*`).
//!
//! * [`Scenario::SandAndWater`] (milestone 3): a 40×32×40 block of sand and one of
//!   water (~100k voxels) side by side, 40 voxels above the ground.
//! * [`Scenario::LavaOnIce`] (milestone 4): a 16×8×16 lava block on a 32×8×32 ice
//!   slab resting on the ground.

use mv_core::WorldPos;
use mv_materials::{MaterialId, MaterialRegistry};
use mv_world::World;
use mv_world::edit::fill_box;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    SandAndWater,
    LavaOnIce,
}

/// A box that was filled, for `Simulation::edited`.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub min: WorldPos,
    pub max: WorldPos,
    pub material: MaterialId,
}

/// Fills the scenario's boxes (only into air) around `ground`.
pub fn spawn(
    world: &mut World,
    registry: &MaterialRegistry,
    ground: WorldPos,
    scenario: Scenario,
) -> Vec<Placed> {
    let g = ground.0;
    let p = |x, y, z| WorldPos::new(g.x + x, g.y + y, g.z + z);
    let boxes: &[(&str, WorldPos, WorldPos)] = match scenario {
        Scenario::SandAndWater => &[
            ("sand", p(-45, 40, -20), p(-6, 71, 19)),
            ("water", p(5, 40, -20), p(44, 71, 19)),
        ],
        Scenario::LavaOnIce => &[
            ("ice", p(-16, 1, -16), p(15, 8, 15)),
            ("lava", p(-8, 9, -8), p(7, 16, 7)),
        ],
    };
    let mut placed = Vec::new();
    let mut voxels = 0;
    for &(name, min, max) in boxes {
        let Some(material) = registry.id(name) else {
            tracing::warn!("scenario {scenario:?} needs the material \"{name}\"");
            continue;
        };
        voxels += fill_box(world, min, max, material, MaterialId::is_air);
        placed.push(Placed { min, max, material });
    }
    tracing::info!("scenario {scenario:?}: placed {voxels} voxels");
    placed
}
