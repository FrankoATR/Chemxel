//! Voxel picking: the same Amanatides & Woo DDA as the GPU raymarcher, on the CPU.
//!
//! Positions are `f64` microvoxels, so picking stays exact far from the origin.
//! Unloaded chunks count as air.

use glam::{DVec3, I64Vec3, IVec3};
use mv_core::{ChunkPos, WorldPos};
use mv_materials::MaterialId;

use crate::{Chunk, World};

/// First non-air voxel along a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub voxel: WorldPos,
    pub material: MaterialId,
    /// Outward normal of the face that was entered (zero if the ray starts inside).
    pub normal: IVec3,
    /// Distance from the ray origin to the entry point, in microvoxels.
    pub distance: f64,
}

impl RayHit {
    /// The empty voxel in front of the hit face (where a block would be placed).
    pub fn adjacent(&self) -> WorldPos {
        WorldPos(self.voxel.0 + self.normal.as_i64vec3())
    }
}

/// Casts a ray from `origin` along `dir` (need not be normalised) up to
/// `max_distance` microvoxels.
pub fn raycast(world: &World, origin: DVec3, dir: DVec3, max_distance: f64) -> Option<RayHit> {
    let dir = dir.try_normalize()?;
    let mut voxel: I64Vec3 = origin.floor().as_i64vec3();
    let step = I64Vec3::new(
        dir.x.signum() as i64,
        dir.y.signum() as i64,
        dir.z.signum() as i64,
    );
    let inv = DVec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
    let delta = inv.abs();
    let next_boundary =
        voxel.as_dvec3() + DVec3::select(dir.cmpgt(DVec3::ZERO), DVec3::ONE, DVec3::ZERO);
    // Axes the ray does not move along must never win the min below (the product
    // would be ±inf or NaN otherwise).
    let mut t_max = DVec3::select(
        dir.cmpeq(DVec3::ZERO),
        DVec3::INFINITY,
        (next_boundary - origin) * inv,
    );
    let mut normal = IVec3::ZERO;
    let mut t = 0.0;

    let mut cached: Option<(ChunkPos, Option<&Chunk>)> = None;
    while t <= max_distance {
        let (cpos, local) = WorldPos(voxel).to_chunk_local();
        let chunk = match cached {
            Some((p, c)) if p == cpos => c,
            _ => {
                let c = world.chunk(cpos);
                cached = Some((cpos, c));
                c
            }
        };
        if let Some(chunk) = chunk {
            let material = chunk.get(local);
            if !material.is_air() {
                return Some(RayHit {
                    voxel: WorldPos(voxel),
                    material,
                    normal,
                    distance: t,
                });
            }
        }
        let axis = if t_max.x < t_max.y && t_max.x < t_max.z {
            0
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        t = t_max[axis];
        t_max[axis] += delta[axis];
        voxel[axis] += step[axis];
        normal = IVec3::ZERO;
        normal[axis] = -step[axis] as i32;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: MaterialId = MaterialId(1);

    fn flat_world() -> World {
        // Ground: y < 0 is stone, around the origin.
        let mut w = World::new();
        for z in -1..=0 {
            for x in -1..=0 {
                w.insert(ChunkPos::new(x, -1, z), Chunk::uniform(STONE));
                w.insert(ChunkPos::new(x, 0, z), Chunk::uniform(MaterialId::AIR));
            }
        }
        w
    }

    #[test]
    fn straight_down_hits_top_face() {
        let w = flat_world();
        let hit = raycast(&w, DVec3::new(-3.5, 10.5, 2.5), DVec3::NEG_Y, 100.0).unwrap();
        assert_eq!(hit.voxel, WorldPos::new(-4, -1, 2));
        assert_eq!(hit.normal, IVec3::Y);
        assert!((hit.distance - 10.5).abs() < 1e-9);
        assert_eq!(hit.adjacent(), WorldPos::new(-4, 0, 2));
        assert_eq!(hit.material, STONE);
    }

    #[test]
    fn diagonal_ray_and_range_limit() {
        let w = flat_world();
        let dir = DVec3::new(1.0, -1.0, 0.0);
        let hit = raycast(&w, DVec3::new(-20.5, 5.5, -3.5), dir, 100.0).unwrap();
        assert_eq!(hit.voxel.0.y, -1);
        assert!(hit.normal == IVec3::Y || hit.normal == IVec3::NEG_X);
        assert!(raycast(&w, DVec3::new(-20.5, 5.5, -3.5), dir, 3.0).is_none());
    }

    #[test]
    fn misses_and_unloaded_chunks() {
        let w = flat_world();
        assert!(raycast(&w, DVec3::new(0.5, 5.5, 0.5), DVec3::Y, 500.0).is_none());
        // Outside the loaded area the ground does not exist.
        assert!(raycast(&w, DVec3::new(500.5, 5.5, 0.5), DVec3::NEG_Y, 50.0).is_none());
        assert!(raycast(&w, DVec3::ZERO, DVec3::ZERO, 10.0).is_none());
    }

    #[test]
    fn starting_inside_solid() {
        let w = flat_world();
        let hit = raycast(&w, DVec3::new(-0.5, -0.5, -0.5), DVec3::X, 10.0).unwrap();
        assert_eq!(hit.voxel, WorldPos::new(-1, -1, -1));
        assert_eq!(hit.normal, IVec3::ZERO);
        assert_eq!(hit.distance, 0.0);
    }
}
