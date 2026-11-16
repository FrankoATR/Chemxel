//! Deterministic test scenes (milestone 1). Procedural terrain from noise replaces
//! these in milestone 2, but they stay useful for tests and benchmarks.

use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry};

use crate::Chunk;

/// Materials used by the test scenes, resolved by name.
#[derive(Debug, Clone, Copy)]
pub struct TestPalette {
    pub stone: MaterialId,
    pub dirt: MaterialId,
    pub grass: MaterialId,
    pub sand: MaterialId,
    pub water: MaterialId,
    pub lava: MaterialId,
}

impl TestPalette {
    /// Resolves the palette; returns the first missing material name on failure.
    pub fn from_registry(registry: &MaterialRegistry) -> Result<Self, &'static str> {
        let get = |name: &'static str| registry.id(name).ok_or(name);
        Ok(Self {
            stone: get("stone")?,
            dirt: get("dirt")?,
            grass: get("grass")?,
            sand: get("sand")?,
            water: get("water")?,
            lava: get("lava")?,
        })
    }
}

/// Height of the sinusoidal test terrain at world column `(x, z)`.
pub fn sine_height(x: i64, z: i64) -> i64 {
    let (x, z) = (x as f64, z as f64);
    (22.0 + 7.0 * (x * 0.13).sin() + 5.0 * (z * 0.09).cos() + 2.0 * ((x + z) * 0.31).sin()) as i64
}

/// Sinusoidal terrain (stone, dirt, grass) with water filling the valleys, a
/// floating sand sphere and a small lava sphere.
pub fn sine_terrain_scene(pos: ChunkPos, p: &TestPalette) -> Chunk {
    const WATER_LEVEL: i64 = 19;
    let sand_center = WorldPos::new(30, 46, 30).0;
    let lava_center = WorldPos::new(50, 34, 44).0;

    Chunk::from_fn(pos, |w| {
        let v = w.0;
        if v.distance_squared(sand_center) <= 11 * 11 {
            return p.sand;
        }
        if v.distance_squared(lava_center) <= 5 * 5 {
            return p.lava;
        }
        let h = sine_height(v.x, v.z);
        match v.y {
            y if y < h - 6 => p.stone,
            y if y < h - 1 => p.dirt,
            y if y < h => p.grass,
            y if y < WATER_LEVEL => p.water,
            _ => MaterialId::AIR,
        }
    })
}

/// A solid sphere of `material` centred in the chunk.
pub fn sphere(pos: ChunkPos, radius: i64, material: MaterialId) -> Chunk {
    let center = pos.origin().0 + 32;
    Chunk::from_fn(pos, |w| {
        if w.0.distance_squared(center) <= radius * radius {
            material
        } else {
            MaterialId::AIR
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_core::LocalPos;

    #[test]
    fn sphere_is_symmetric() {
        let stone = MaterialId(1);
        let c = sphere(ChunkPos::new(0, 0, 0), 10, stone);
        assert_eq!(c.get(LocalPos::new(32, 32, 32)), stone);
        assert_eq!(c.get(LocalPos::new(42, 32, 32)), stone);
        assert_eq!(c.get(LocalPos::new(22, 32, 32)), stone);
        assert_eq!(c.get(LocalPos::new(43, 32, 32)), MaterialId::AIR);
        assert_eq!(c.get(LocalPos::new(0, 0, 0)), MaterialId::AIR);
    }

    #[test]
    fn scene_is_deterministic() {
        let p = TestPalette {
            stone: MaterialId(1),
            dirt: MaterialId(2),
            grass: MaterialId(3),
            sand: MaterialId(4),
            water: MaterialId(5),
            lava: MaterialId(6),
        };
        let a = sine_terrain_scene(ChunkPos::new(0, 0, 0), &p);
        let b = sine_terrain_scene(ChunkPos::new(0, 0, 0), &p);
        assert_eq!(a.data(), b.data());
        assert_eq!(a.get(LocalPos::new(10, 0, 10)), p.stone);
        assert_eq!(a.get(LocalPos::new(10, 63, 10)), MaterialId::AIR);
    }
}
