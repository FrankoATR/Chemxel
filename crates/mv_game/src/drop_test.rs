//! Milestone 3 acceptance scenario in the live game: drops a 40×32×40 block of sand
//! and one of water (~100k voxels) above a point, side by side.

use mv_core::WorldPos;
use mv_materials::{MaterialId, MaterialRegistry};
use mv_world::World;
use mv_world::edit::fill_box;

/// Boxes `(min, max)` that were filled (to wake the simulation there).
pub fn drop_sand_and_water(
    world: &mut World,
    registry: &MaterialRegistry,
    ground: WorldPos,
) -> Vec<(WorldPos, WorldPos)> {
    let (Some(sand), Some(water)) = (registry.id("sand"), registry.id("water")) else {
        tracing::warn!("drop test needs the sand and water materials");
        return Vec::new();
    };
    let g = ground.0;
    let y0 = g.y + 40;
    let boxes = [
        (
            sand,
            WorldPos::new(g.x - 45, y0, g.z - 20),
            WorldPos::new(g.x - 6, y0 + 31, g.z + 19),
        ),
        (
            water,
            WorldPos::new(g.x + 5, y0, g.z - 20),
            WorldPos::new(g.x + 44, y0 + 31, g.z + 19),
        ),
    ];
    let mut filled = Vec::new();
    let mut voxels = 0;
    for (material, min, max) in boxes {
        voxels += fill_box(world, min, max, material, MaterialId::is_air);
        filled.push((min, max));
    }
    tracing::info!("drop test: placed {voxels} voxels of sand and water");
    filled
}
