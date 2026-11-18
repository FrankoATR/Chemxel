//! Test scenarios spawned in the live game (overlay buttons and `--bench-*`).
//!
//! * [`Scenario::SandAndWater`] (milestone 3): a 40×32×40 block of sand and one of
//!   water (~100k voxels) side by side, 40 voxels above the ground.
//! * [`Scenario::LavaOnIce`] (milestone 4): a 16×8×16 lava block on a 32×8×32 ice
//!   slab resting on the ground.
//! * [`Scenario::ForestFire`] (milestone 5): a 6×6 grid of trees with touching
//!   canopies on the terrain, lit at one corner.
//! * [`Scenario::TowerCollapse`] (milestone 6): a 1×6 m stone tower whose base is
//!   dug out, half of it over a pit, so it falls and tips over.
//!
//! Boxes of material "air" are removals (they overwrite anything); the caller
//! reports them to the simulation as such, which triggers structural integrity.

use mv_core::WorldPos;
use mv_materials::{MaterialId, MaterialRegistry};
use mv_world::World;
use mv_world::edit::fill_box;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    SandAndWater,
    LavaOnIce,
    ForestFire,
    TowerCollapse,
}

/// A box that was filled, for `Simulation::edited`.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub min: WorldPos,
    pub max: WorldPos,
    pub material: MaterialId,
}

/// Fills the scenario's boxes (only into air) around `ground`, in order.
pub fn spawn(
    world: &mut World,
    registry: &MaterialRegistry,
    ground: WorldPos,
    scenario: Scenario,
) -> Vec<Placed> {
    let g = ground.0;
    let p = |x, y, z| WorldPos::new(g.x + x, g.y + y, g.z + z);
    let boxes: Vec<(&str, WorldPos, WorldPos)> = match scenario {
        Scenario::SandAndWater => vec![
            ("sand", p(-45, 40, -20), p(-6, 71, 19)),
            ("water", p(5, 40, -20), p(44, 71, 19)),
        ],
        Scenario::LavaOnIce => vec![
            ("ice", p(-16, 1, -16), p(15, 8, 15)),
            ("lava", p(-8, 9, -8), p(7, 16, 7)),
        ],
        Scenario::ForestFire => forest(world, ground),
        Scenario::TowerCollapse => tower(world, ground),
    };
    let mut placed = Vec::new();
    let mut voxels = 0;
    for (name, min, max) in boxes {
        let Some(material) = registry.id(name) else {
            tracing::warn!("scenario {scenario:?} needs the material \"{name}\"");
            continue;
        };
        voxels += if material.is_air() {
            fill_box(world, min, max, material, |_| true)
        } else {
            fill_box(world, min, max, material, MaterialId::is_air)
        };
        placed.push(Placed { min, max, material });
    }
    tracing::info!("scenario {scenario:?}: placed {voxels} voxels");
    placed
}

/// Trees per side of the forest grid.
const FOREST_SIDE: i64 = 6;
/// Distance between trunks; canopies are 13 wide, so they touch.
const TREE_SPACING: i64 = 12;

/// Trunks, then canopies, then the fire, so each fills only air.
fn forest(world: &World, ground: WorldPos) -> Vec<(&'static str, WorldPos, WorldPos)> {
    let g = ground.0;
    let half = FOREST_SIDE * TREE_SPACING / 2;
    let mut trunks = Vec::new();
    let mut canopies = Vec::new();
    let mut fire = Vec::new();
    for i in 0..FOREST_SIDE {
        for j in 0..FOREST_SIDE {
            // Deterministic jitter so the forest does not look like a grid.
            let h = mix(i, j);
            let x = g.x - half + i * TREE_SPACING + (h % 5) as i64 - 2;
            let z = g.z - half + j * TREE_SPACING + ((h >> 8) % 5) as i64 - 2;
            let Some(s) = surface(world, x, z, g.y) else {
                continue;
            };
            let top = s + 32 + ((h >> 16) % 13) as i64;
            let w = |x, y, z| WorldPos::new(x, y, z);
            trunks.push(("wood", w(x - 1, s + 1, z - 1), w(x + 1, top, z + 1)));
            canopies.push(("leaves", w(x - 6, top - 6, z - 6), w(x + 6, top - 1, z + 6)));
            canopies.push(("leaves", w(x - 4, top, z - 4), w(x + 4, top + 3, z + 4)));
            if i == 0 && j == 0 {
                fire.push(("fire", w(x - 4, s + 1, z - 1), w(x - 2, s + 6, z + 1)));
            }
        }
    }
    trunks.into_iter().chain(canopies).chain(fire).collect()
}

/// A 16×96×16 stone tower standing on the ground at `ground`, then its base dug out
/// and a pit under one half, so it drops and tips into the pit.
fn tower(world: &World, ground: WorldPos) -> Vec<(&'static str, WorldPos, WorldPos)> {
    let g = ground.0;
    let s = surface(world, g.x, g.z, g.y).unwrap_or(g.y);
    let w = |x, y, z| WorldPos::new(g.x + x, y, g.z + z);
    vec![
        ("stone", w(-8, s + 1, -8), w(7, s + 96, 7)),
        // The pit under the +x half (and a bit beyond), then the base itself.
        ("air", w(0, s - 12, -10), w(30, s, 9)),
        ("air", w(-8, s + 1, -8), w(7, s + 6, 7)),
    ]
}

/// Height of the topmost non-air voxel of column `(x, z)` within 64 voxels of `y`.
fn surface(world: &World, x: i64, z: i64, y: i64) -> Option<i64> {
    ((y - 64)..=(y + 64)).rev().find(|&y| {
        world
            .voxel(WorldPos::new(x, y, z))
            .is_some_and(|m| !m.is_air())
    })
}

fn mix(i: i64, j: i64) -> u64 {
    let mut h =
        (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (j as u64).wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}
