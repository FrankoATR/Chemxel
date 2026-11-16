//! Procedural heightmap terrain.
//!
//! The surface height of every world column comes from three fBm noise layers:
//! rolling hills, small detail and ridged mountains masked to some regions. Each
//! column is then layered (top to bottom) as grass, dirt and stone, with sand on
//! beaches and under water, and water filling everything below the sea level.
//!
//! Generation works per chunk but the 64×64 heights of a chunk column are computed
//! once and cached, because every chunk of the column needs them. Whole chunks and
//! whole bricks that are provably uniform (deep stone, open air, open water) are
//! classified from the column's min/max height without touching individual voxels.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::IVec2;
use mv_core::config::{TerrainConfig, WorldConfig};
use mv_core::consts::{BRICK_SIZE, BRICK_VOLUME, BRICKS_PER_CHUNK_AXIS, CHUNK_SIZE};
use mv_core::{ChunkPos, LocalPos};
use mv_materials::{MaterialId, MaterialRegistry};
use noise::{Fbm, MultiFractal, NoiseFn, Perlin, RidgedMulti};

use crate::{Brick, Chunk};

/// Materials used by the terrain, resolved by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainPalette {
    pub stone: MaterialId,
    pub dirt: MaterialId,
    pub grass: MaterialId,
    pub sand: MaterialId,
    pub water: MaterialId,
}

impl TerrainPalette {
    /// Resolves the palette; returns the first missing material name on failure.
    pub fn from_registry(registry: &MaterialRegistry) -> Result<Self, &'static str> {
        let get = |name: &'static str| registry.id(name).ok_or(name);
        Ok(Self {
            stone: get("stone")?,
            dirt: get("dirt")?,
            grass: get("grass")?,
            sand: get("sand")?,
            water: get("water")?,
        })
    }
}

const COLUMN_AREA: usize = CHUNK_SIZE * CHUNK_SIZE;
const BRICK_COLUMNS: usize = BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS;

/// Surface heights of one chunk column. `heights[x + z*64]` is the height of the
/// column: voxels with `y < height` are ground.
#[derive(Debug)]
pub struct ColumnHeights {
    heights: Box<[i32; COLUMN_AREA]>,
    min: i32,
    max: i32,
    /// Min/max height per 8×8 brick column, indexed `bx + bz*8`.
    brick_min: [i32; BRICK_COLUMNS],
    brick_max: [i32; BRICK_COLUMNS],
}

impl ColumnHeights {
    #[inline]
    pub fn get(&self, x: usize, z: usize) -> i32 {
        self.heights[x + z * CHUNK_SIZE]
    }

    pub fn min(&self) -> i32 {
        self.min
    }

    pub fn max(&self) -> i32 {
        self.max
    }
}

/// Deterministic terrain generator. Cheap to share between threads (`Arc`).
#[derive(Debug)]
pub struct TerrainGenerator {
    config: TerrainConfig,
    sea_level: i32,
    palette: TerrainPalette,
    hills: Fbm<Perlin>,
    detail: Fbm<Perlin>,
    mountains: RidgedMulti<Perlin>,
    mountain_mask: Fbm<Perlin>,
    columns: Mutex<HashMap<IVec2, Arc<ColumnHeights>>>,
}

impl TerrainGenerator {
    pub fn new(config: &WorldConfig, seed: u64, palette: TerrainPalette) -> Self {
        let t = config.terrain.clone();
        // Distinct, deterministic seeds per layer.
        let s = |k: u64| (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(k) >> 16) as u32;
        let octaves = t.octaves as usize;
        let fbm = |seed: u32, wavelength: f64, octaves: usize| {
            Fbm::<Perlin>::new(seed)
                .set_octaves(octaves)
                .set_frequency(1.0 / wavelength)
                .set_persistence(0.5)
                .set_lacunarity(2.0)
        };
        Self {
            hills: fbm(s(1), t.hills_wavelength, octaves),
            detail: fbm(s(2), t.detail_wavelength, 3),
            mountains: RidgedMulti::<Perlin>::new(s(3))
                .set_octaves(octaves)
                .set_frequency(1.0 / t.mountain_wavelength)
                .set_lacunarity(2.0),
            mountain_mask: fbm(s(4), t.mountain_wavelength * 2.5, 2),
            config: t,
            sea_level: config.sea_level,
            palette,
            columns: Mutex::new(HashMap::new()),
        }
    }

    pub fn palette(&self) -> TerrainPalette {
        self.palette
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// Surface height of the world column `(x, z)`.
    pub fn height_at(&self, x: i64, z: i64) -> i32 {
        let t = &self.config;
        let p = [x as f64, z as f64];
        let hills = self.hills.get(p) * t.hills_amplitude;
        let detail = self.detail.get(p) * t.detail_amplitude;
        // Mountains only where the low-frequency mask is high.
        let mask = smoothstep(0.05, 0.45, self.mountain_mask.get(p));
        // Ridged noise can exceed 1; saturate smoothly instead of clamping, which
        // would cut flat plateaus into the peaks.
        let r = (self.mountains.get(p) * 0.5 + 0.5).max(0.0);
        let ridges = 1.0 - (-1.6 * r).exp();
        let mountains = mask * ridges * ridges * t.mountain_amplitude;
        (t.base_height + hills + detail + mountains).floor() as i32
    }

    /// Material of the voxel at height `y` in a column whose surface is `height`.
    #[inline]
    pub fn column_material(&self, height: i32, y: i32) -> MaterialId {
        let t = &self.config;
        let p = &self.palette;
        if y < height - t.dirt_depth {
            p.stone
        } else if y < height {
            if height <= self.sea_level + t.beach_height {
                p.sand
            } else if y >= height - t.grass_depth {
                p.grass
            } else {
                p.dirt
            }
        } else if y < self.sea_level {
            p.water
        } else {
            MaterialId::AIR
        }
    }

    fn compute_column(&self, column: IVec2) -> ColumnHeights {
        let _span = tracing::trace_span!("terrain_column").entered();
        let x0 = i64::from(column.x) * CHUNK_SIZE as i64;
        let z0 = i64::from(column.y) * CHUNK_SIZE as i64;
        let mut heights = Box::new([0i32; COLUMN_AREA]);
        let mut brick_min = [i32::MAX; BRICK_COLUMNS];
        let mut brick_max = [i32::MIN; BRICK_COLUMNS];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = self.height_at(x0 + x as i64, z0 + z as i64);
                heights[x + z * CHUNK_SIZE] = h;
                let b = x / BRICK_SIZE + (z / BRICK_SIZE) * BRICKS_PER_CHUNK_AXIS;
                brick_min[b] = brick_min[b].min(h);
                brick_max[b] = brick_max[b].max(h);
            }
        }
        ColumnHeights {
            min: brick_min.iter().copied().min().unwrap_or(0),
            max: brick_max.iter().copied().max().unwrap_or(0),
            heights,
            brick_min,
            brick_max,
        }
    }

    /// Heights of a chunk column, computed once and cached.
    pub fn column(&self, column: IVec2) -> Arc<ColumnHeights> {
        if let Some(c) = self.lock_columns().get(&column) {
            return Arc::clone(c);
        }
        // Computed outside the lock; two threads may race to compute the same column,
        // which only wastes a little work.
        let computed = Arc::new(self.compute_column(column));
        Arc::clone(self.lock_columns().entry(column).or_insert(computed))
    }

    /// Drops cached columns farther than `radius` chunks from `center` (horizontally).
    pub fn retain_columns(&self, center: IVec2, radius: i32) {
        self.lock_columns()
            .retain(|c, _| (*c - center).abs().max_element() <= radius);
    }

    pub fn cached_columns(&self) -> usize {
        self.lock_columns().len()
    }

    fn lock_columns(&self) -> std::sync::MutexGuard<'_, HashMap<IVec2, Arc<ColumnHeights>>> {
        // A panic while holding the lock cannot leave the cache inconsistent (entries
        // are inserted whole), so a poisoned lock is still usable.
        self.columns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Uniform material for the vertical span `[y0, y1)` over columns with heights in
    /// `[min_h, max_h]`, if it can be decided without looking at each column.
    fn classify_span(&self, min_h: i32, max_h: i32, y0: i32, y1: i32) -> Option<MaterialId> {
        if y1 <= min_h - self.config.dirt_depth {
            return Some(self.palette.stone);
        }
        if y0 >= max_h {
            if y0 >= self.sea_level {
                return Some(MaterialId::AIR);
            }
            if y1 <= self.sea_level {
                return Some(self.palette.water);
            }
        }
        None
    }

    /// Generates the chunk at `pos`.
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let _span = tracing::trace_span!("terrain_chunk").entered();
        let column = self.column(IVec2::new(pos.0.x, pos.0.z));
        let cy0 = pos.0.y * CHUNK_SIZE as i32;
        if let Some(m) = self.classify_span(column.min, column.max, cy0, cy0 + CHUNK_SIZE as i32) {
            return Chunk::uniform(m);
        }

        let mut scratch = [MaterialId::AIR; BRICK_VOLUME];
        let bricks = Box::new(std::array::from_fn(|brick| {
            let bx = brick % BRICKS_PER_CHUNK_AXIS;
            let by = (brick / BRICKS_PER_CHUNK_AXIS) % BRICKS_PER_CHUNK_AXIS;
            let bz = brick / (BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS);
            let bcol = bx + bz * BRICKS_PER_CHUNK_AXIS;
            let y0 = cy0 + (by * BRICK_SIZE) as i32;
            if let Some(m) = self.classify_span(
                column.brick_min[bcol],
                column.brick_max[bcol],
                y0,
                y0 + BRICK_SIZE as i32,
            ) {
                return Brick::Uniform(m);
            }
            for (local, voxel) in scratch.iter_mut().enumerate() {
                let p = LocalPos::from_brick(brick, local);
                let h = column.get(p.x.into(), p.z.into());
                *voxel = self.column_material(h, cy0 + i32::from(p.y));
            }
            Brick::from_voxels(&scratch)
        }));
        Chunk::from_bricks(bricks)
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_core::WorldPos;

    fn palette() -> TerrainPalette {
        TerrainPalette {
            stone: MaterialId(1),
            dirt: MaterialId(2),
            grass: MaterialId(3),
            sand: MaterialId(4),
            water: MaterialId(5),
        }
    }

    fn generator(seed: u64) -> TerrainGenerator {
        TerrainGenerator::new(&WorldConfig::default(), seed, palette())
    }

    /// Reference implementation: evaluates every voxel independently.
    fn reference(g: &TerrainGenerator, pos: ChunkPos) -> Chunk {
        Chunk::from_fn(pos, |w: WorldPos| {
            g.column_material(g.height_at(w.0.x, w.0.z), w.0.y as i32)
        })
    }

    #[test]
    fn fast_paths_match_reference() {
        let g = generator(7);
        // Find a column, then test every chunk from deep stone to open air.
        let h = g.height_at(0, 0);
        let surface = h.div_euclid(CHUNK_SIZE as i32);
        for cx in [-1, 0] {
            for cy in (surface - 2)..=(surface + 2) {
                let pos = ChunkPos::new(cx, cy, 0);
                assert_eq!(
                    g.generate(pos).data(),
                    reference(&g, pos).data(),
                    "mismatch at {pos:?}"
                );
            }
        }
        // Sea-level chunks too (water/air interfaces).
        for cx in -3..3 {
            let pos = ChunkPos::new(cx * 7, 0, cx * 5);
            assert_eq!(g.generate(pos).data(), reference(&g, pos).data());
            let pos = ChunkPos::new(cx * 7, -1, cx * 5);
            assert_eq!(g.generate(pos).data(), reference(&g, pos).data());
        }
    }

    #[test]
    fn deterministic_per_seed() {
        let a = generator(1);
        let b = generator(1);
        let c = generator(2);
        let pos = ChunkPos::new(3, 0, -2);
        assert_eq!(a.generate(pos).data(), b.generate(pos).data());
        let differs = (0..64).any(|i| a.height_at(i * 97, i * 31) != c.height_at(i * 97, i * 31));
        assert!(differs, "different seeds should give different terrain");
    }

    #[test]
    fn column_layers() {
        let g = generator(0);
        let p = palette();
        let t = TerrainConfig::default();
        let h = 100; // well above the beach
        assert_eq!(g.column_material(h, h), MaterialId::AIR);
        assert_eq!(g.column_material(h, h - 1), p.grass);
        assert_eq!(g.column_material(h, h - t.grass_depth - 1), p.dirt);
        assert_eq!(g.column_material(h, h - t.dirt_depth - 1), p.stone);
        // Under water: sand floor, water above it up to the sea level.
        let h = -20;
        assert_eq!(g.column_material(h, h - 1), p.sand);
        assert_eq!(g.column_material(h, h), p.water);
        assert_eq!(g.column_material(h, -1), p.water);
        assert_eq!(g.column_material(h, 0), MaterialId::AIR);
    }

    #[test]
    fn far_chunks_are_uniform() {
        let g = generator(3);
        assert_eq!(
            g.generate(ChunkPos::new(0, -40, 0)).as_uniform(),
            Some(palette().stone)
        );
        assert_eq!(
            g.generate(ChunkPos::new(0, 40, 0)).as_uniform(),
            Some(MaterialId::AIR)
        );
    }

    #[test]
    fn column_cache_retains_nearby() {
        let g = generator(0);
        g.column(IVec2::new(0, 0));
        g.column(IVec2::new(10, 0));
        assert_eq!(g.cached_columns(), 2);
        g.retain_columns(IVec2::ZERO, 5);
        assert_eq!(g.cached_columns(), 1);
    }
}
