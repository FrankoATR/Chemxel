//! Procedural terrain (ADR 013).
//!
//! The surface height of every world column comes from fBm noise layers: rolling
//! hills, small detail, ridged mountains masked to some regions and, with generator
//! 2, fine roughness (breaks the long contour lines of gentle slopes) and stepped
//! cliffs. Each column is then layered top to bottom: grass over dirt over stone,
//! sand on beaches and under water, bare rock with strata where the slope is steep,
//! and water below the sea level. Meadow columns may carry a tuft of tall grass,
//! some with a flower on top.
//!
//! Trees and boulders are *features*: a few capsules and noisy blobs placed
//! deterministically, one candidate per grid cell, so any chunk can tell which of
//! them reach into it without knowing about its neighbours.
//!
//! Generation works per chunk but the 64×64 columns of a chunk column are computed
//! once and cached, because every chunk of the column needs them. Whole chunks and
//! whole bricks that are provably uniform (deep stone, open air, open water) are
//! classified from the column's min/max height without touching individual voxels.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};

use glam::{I64Vec3, IVec2, Vec3};
use mv_core::config::{TerrainConfig, WorldConfig};
use mv_core::consts::{BRICK_SIZE, BRICK_VOLUME, BRICKS_PER_CHUNK_AXIS, CHUNK_SIZE};
use mv_core::{ChunkPos, LocalPos, WorldPos};
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
    /// Strata of bare rock.
    pub sandstone: MaterialId,
    pub basalt: MaterialId,
    /// Vegetation on meadows.
    pub tall_grass: MaterialId,
    pub flowers: [MaterialId; 3],
    /// Trees: wood, and green / autumn / golden foliage.
    pub wood: MaterialId,
    pub leaves: [MaterialId; 3],
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
            sandstone: get("sandstone")?,
            basalt: get("basalt")?,
            tall_grass: get("tall_grass")?,
            flowers: [
                get("flower_red")?,
                get("flower_yellow")?,
                get("flower_violet")?,
            ],
            wood: get("wood")?,
            leaves: [get("leaves")?, get("leaves_autumn")?, get("leaves_gold")?],
        })
    }

    /// Distinct ids 1..=16 in the order of the fields, for tests without a registry.
    pub fn synthetic() -> Self {
        let m = MaterialId;
        Self {
            stone: m(1),
            dirt: m(2),
            grass: m(3),
            sand: m(4),
            water: m(5),
            sandstone: m(6),
            basalt: m(7),
            tall_grass: m(8),
            flowers: [m(9), m(10), m(11)],
            wood: m(12),
            leaves: [m(13), m(14), m(15)],
        }
    }

    /// Whether a terrain feature (tree, boulder) may replace `m`.
    fn replaceable(&self, m: MaterialId) -> bool {
        m.is_air() || m == self.tall_grass || self.flowers.contains(&m)
    }
}

const COLUMN_AREA: usize = CHUNK_SIZE * CHUNK_SIZE;
const BRICK_COLUMNS: usize = BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS;
/// Width of the height grid of a chunk column, with a one-column border for slopes.
const GRID: usize = CHUNK_SIZE + 2;
/// Farthest a tree reaches horizontally from its trunk (voxels).
const TREE_REACH: i64 = 100;
/// Highest a feature reaches above the highest column of a chunk column it can
/// touch (a tree on a cliff top next to it), and lowest below the lowest.
const FEATURE_ABOVE: i32 = 256;
const FEATURE_BELOW: i32 = 64;
/// Side of the cells boulders are placed in.
const BOULDER_CELL: i64 = 96;
const BOULDER_REACH: i64 = 24;
/// Bands of bare rock, from `y / STRATUM_THICKNESS`: 0 sandstone, 1 stone, 2 basalt.
const STRATA: [u8; 6] = [0, 0, 1, 0, 2, 1];
const STRATUM_THICKNESS: i32 = 9;

const SALT_GRASS: u64 = 0x0067_7261_7373;
const SALT_TREE: u64 = 0x7472_6565;
const SALT_BOULDER: u64 = 0x726f_636b;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Surface {
    #[default]
    Grass,
    Sand,
    Rock,
}

/// One world column: its surface and what grows on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Column {
    /// Voxels with `y < height` are ground.
    pub height: i32,
    surface: Surface,
    /// Height of the tuft of vegetation on top (0: none).
    decor: u8,
    /// Flower at the top of the tuft: 0 none, else index + 1 into the palette.
    flower: u8,
    /// Vertical offset of the rock strata (they wave across the cliff).
    strata: i8,
}

/// Columns of one chunk column, `x + z*64`.
#[derive(Debug)]
pub struct ColumnHeights {
    columns: Box<[Column; COLUMN_AREA]>,
    min: i32,
    max: i32,
    /// Min surface height, and max height of the surface plus vegetation, per 8×8
    /// brick column, indexed `bx + bz*8`.
    brick_min: [i32; BRICK_COLUMNS],
    brick_top: [i32; BRICK_COLUMNS],
}

impl ColumnHeights {
    #[inline]
    pub fn get(&self, x: usize, z: usize) -> i32 {
        self.columns[x + z * CHUNK_SIZE].height
    }

    #[inline]
    pub fn column(&self, x: usize, z: usize) -> &Column {
        &self.columns[x + z * CHUNK_SIZE]
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
    seed: u64,
    /// Generator 2 (natural landscape); false: the plain heightmap of generator 1.
    natural: bool,
    hills: Fbm<Perlin>,
    detail: Fbm<Perlin>,
    mountains: RidgedMulti<Perlin>,
    mountain_mask: Fbm<Perlin>,
    roughness: Fbm<Perlin>,
    cliffs: Fbm<Perlin>,
    cliff_mask: Fbm<Perlin>,
    /// Low-frequency fields: meadows, flower patches and their colour, strata
    /// offset, forests and autumn regions.
    meadow: Perlin,
    flower_patch: Perlin,
    flower_hue: Perlin,
    strata: Perlin,
    forest: Perlin,
    autumn: Perlin,
    columns: Mutex<HashMap<IVec2, Arc<ColumnHeights>>>,
}

impl TerrainGenerator {
    pub fn new(config: &WorldConfig, seed: u64, palette: TerrainPalette) -> Self {
        let mut t = config.terrain.clone();
        let natural = t.generator >= 2;
        if !natural {
            // Generator 1 ignores the landscape settings.
            t.roughness_amplitude = 0.0;
            t.cliff_amplitude = 0.0;
            t.rock_slope = i32::MAX;
            t.grass_density = 0.0;
            t.tree_density = 0.0;
            t.boulder_density = 0.0;
        }
        t.tree_spacing = t.tree_spacing.max(32);
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
            roughness: fbm(s(5), t.roughness_wavelength.max(1.0), 2),
            cliffs: fbm(s(6), t.cliff_wavelength.max(1.0), 3),
            cliff_mask: fbm(s(7), t.cliff_wavelength.max(1.0) * 2.0, 2),
            meadow: Perlin::new(s(8)),
            flower_patch: Perlin::new(s(9)),
            flower_hue: Perlin::new(s(10)),
            strata: Perlin::new(s(11)),
            forest: Perlin::new(s(12)),
            autumn: Perlin::new(s(13)),
            config: t,
            sea_level: config.sea_level,
            palette,
            seed,
            natural,
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
        let mut h = t.base_height + hills + detail + mountains;
        if self.natural {
            h += self.roughness.get(p) * t.roughness_amplitude + self.cliff_height(p);
        }
        h.floor() as i32
    }

    /// Stepped cliffs: plateaus `cliff_amplitude / 4` apart joined by steep risers,
    /// where the mask allows.
    fn cliff_height(&self, p: [f64; 2]) -> f64 {
        let amplitude = self.config.cliff_amplitude;
        if amplitude <= 0.0 {
            return 0.0;
        }
        let mask = smoothstep(0.0, 0.35, self.cliff_mask.get(p));
        if mask <= 0.0 {
            return 0.0;
        }
        // fBm stays mostly within ±0.5: about six steps across its range.
        let s = self.cliffs.get(p) * 6.0;
        let stepped = s.floor() + smoothstep(0.9, 1.0, s.fract());
        mask * stepped * amplitude / 4.0
    }

    /// The column at `(x, z)` with surface `height`, given the largest height
    /// difference to its four neighbours.
    fn column_from(&self, x: i64, z: i64, height: i32, slope: i32) -> Column {
        let t = &self.config;
        let surface = if height <= self.sea_level + t.beach_height {
            Surface::Sand
        } else if slope >= t.rock_slope {
            Surface::Rock
        } else {
            Surface::Grass
        };
        let mut c = Column {
            height,
            surface,
            ..Column::default()
        };
        if !self.natural {
            return c;
        }
        let p = [x as f64, z as f64];
        if surface == Surface::Rock {
            c.strata = (self.strata.get(scaled(p, 160.0)) * 10.0) as i8;
        }
        if surface == Surface::Grass && slope <= 2 && t.grass_density > 0.0 {
            let r = hash2(self.seed ^ SALT_GRASS, x, z);
            let meadow = smoothstep(-0.35, 0.35, self.meadow.get(scaled(p, 240.0)));
            if unit(r) < t.grass_density * meadow {
                // Mostly short tufts (1–3 voxels), a quarter of them taller.
                c.decor =
                    1 + ((r >> 24) % 3) as u8 + if (r >> 28).is_multiple_of(4) { 2 } else { 0 };
                let patch = smoothstep(0.1, 0.5, self.flower_patch.get(scaled(p, 130.0)));
                if unit(r >> 32) < t.flower_chance * patch {
                    let hue = self.flower_hue.get(scaled(p, 90.0)) + (unit(r >> 40) - 0.5) * 0.3;
                    c.flower = if hue < -0.2 {
                        1
                    } else if hue < 0.2 {
                        2
                    } else {
                        3
                    };
                    c.decor += 1;
                }
            }
        }
        c
    }

    /// The column at `(x, z)` and its slope, evaluated on its own (no cache).
    pub fn column_at(&self, x: i64, z: i64) -> Column {
        self.column_and_slope(x, z).0
    }

    fn column_and_slope(&self, x: i64, z: i64) -> (Column, i32) {
        let h = self.height_at(x, z);
        let slope = if self.natural {
            [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .map(|&(dx, dz)| (self.height_at(x + dx, z + dz) - h).abs())
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        (self.column_from(x, z, h, slope), slope)
    }

    /// Material of the voxel at height `y` of column `c` (features aside).
    #[inline]
    pub fn column_material(&self, c: &Column, y: i32) -> MaterialId {
        let t = &self.config;
        let p = &self.palette;
        let h = c.height;
        if y < h {
            let depth = h - 1 - y;
            if depth >= t.dirt_depth {
                return p.stone;
            }
            match c.surface {
                Surface::Sand => p.sand,
                Surface::Rock => self.stratum(y + i32::from(c.strata)),
                Surface::Grass if depth < t.grass_depth => p.grass,
                Surface::Grass => p.dirt,
            }
        } else if y < h + i32::from(c.decor) {
            if c.flower != 0 && y == h + i32::from(c.decor) - 1 {
                p.flowers[usize::from(c.flower - 1)]
            } else {
                p.tall_grass
            }
        } else if y < self.sea_level {
            p.water
        } else {
            MaterialId::AIR
        }
    }

    fn stratum(&self, y: i32) -> MaterialId {
        let band = y
            .div_euclid(STRATUM_THICKNESS)
            .rem_euclid(STRATA.len() as i32);
        match STRATA[band as usize] {
            0 => self.palette.sandstone,
            1 => self.palette.stone,
            _ => self.palette.basalt,
        }
    }

    fn compute_column(&self, column: IVec2) -> ColumnHeights {
        let _span = tracing::trace_span!("terrain_column").entered();
        let x0 = i64::from(column.x) * CHUNK_SIZE as i64;
        let z0 = i64::from(column.y) * CHUNK_SIZE as i64;
        // Heights with a one-column border (slopes need the neighbours); generator 1
        // needs no slopes and skips the border.
        let mut grid = vec![0i32; GRID * GRID];
        for gz in 0..GRID {
            for gx in 0..GRID {
                let border = gx == 0 || gz == 0 || gx == GRID - 1 || gz == GRID - 1;
                if border && !self.natural {
                    continue;
                }
                grid[gx + gz * GRID] = self.height_at(x0 + gx as i64 - 1, z0 + gz as i64 - 1);
            }
        }
        let mut columns = Box::new([Column::default(); COLUMN_AREA]);
        let mut brick_min = [i32::MAX; BRICK_COLUMNS];
        let mut brick_top = [i32::MIN; BRICK_COLUMNS];
        let mut max = i32::MIN;
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let g = (x + 1) + (z + 1) * GRID;
                let h = grid[g];
                let slope = if self.natural {
                    [g - 1, g + 1, g - GRID, g + GRID]
                        .iter()
                        .map(|&n| (grid[n] - h).abs())
                        .max()
                        .unwrap_or(0)
                } else {
                    0
                };
                let c = self.column_from(x0 + x as i64, z0 + z as i64, h, slope);
                columns[x + z * CHUNK_SIZE] = c;
                let b = x / BRICK_SIZE + (z / BRICK_SIZE) * BRICKS_PER_CHUNK_AXIS;
                brick_min[b] = brick_min[b].min(h);
                brick_top[b] = brick_top[b].max(h + i32::from(c.decor));
                max = max.max(h);
            }
        }
        ColumnHeights {
            min: brick_min.iter().copied().min().unwrap_or(0),
            max,
            columns,
            brick_min,
            brick_top,
        }
    }

    /// Columns of a chunk column, computed once and cached.
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

    /// Uniform material for the vertical span `[y0, y1)` over columns with surface
    /// heights from `min_h` and surface-plus-vegetation up to `top`, if it can be
    /// decided without looking at each column. With `features` in the span, only
    /// deep stone (which features never replace) is uniform.
    fn classify_span(
        &self,
        min_h: i32,
        top: i32,
        y0: i32,
        y1: i32,
        features: bool,
    ) -> Option<MaterialId> {
        if y1 <= min_h - self.config.dirt_depth {
            return Some(self.palette.stone);
        }
        if y0 >= top && !features {
            if y0 >= self.sea_level {
                return Some(MaterialId::AIR);
            }
            if y1 <= self.sea_level {
                return Some(self.palette.water);
            }
        }
        None
    }

    /// Features (trees, boulders) that may reach into the box `[min, max]`, appended
    /// to `out`.
    pub fn features(&self, min: I64Vec3, max: I64Vec3, out: &mut Vec<Feature>) {
        if !self.natural {
            return;
        }
        let start = out.len();
        if self.config.tree_density > 0.0 {
            let s = i64::from(self.config.tree_spacing);
            for cz in (min.z - TREE_REACH).div_euclid(s)..=(max.z + TREE_REACH).div_euclid(s) {
                for cx in (min.x - TREE_REACH).div_euclid(s)..=(max.x + TREE_REACH).div_euclid(s) {
                    self.tree(cx, cz, out);
                }
            }
        }
        if self.config.boulder_density > 0.0 {
            let s = BOULDER_CELL;
            for cz in (min.z - BOULDER_REACH).div_euclid(s)..=(max.z + BOULDER_REACH).div_euclid(s)
            {
                for cx in
                    (min.x - BOULDER_REACH).div_euclid(s)..=(max.x + BOULDER_REACH).div_euclid(s)
                {
                    self.boulder(cx, cz, out);
                }
            }
        }
        let mut i = start;
        while i < out.len() {
            if out[i].overlaps(min, max) {
                i += 1;
            } else {
                out.swap_remove(i);
            }
        }
    }

    /// The tree of grid cell `(cx, cz)`, if it has one: a leaning two-segment trunk,
    /// 3–5 branches and clumps of foliage at their ends and on top.
    fn tree(&self, cx: i64, cz: i64, out: &mut Vec<Feature>) {
        let t = &self.config;
        let p = &self.palette;
        let s = i64::from(t.tree_spacing);
        let r = hash2(self.seed ^ SALT_TREE, cx, cz);
        let centre = [(cx as f64 + 0.5) * s as f64, (cz as f64 + 0.5) * s as f64];
        let forest = smoothstep(-0.15, 0.35, self.forest.get(scaled(centre, 900.0)));
        if unit(r) >= t.tree_density * forest {
            return;
        }
        let margin = 12;
        let span = (s - 2 * margin).max(1) as u64;
        let x = cx * s + margin + ((r >> 24) % span) as i64;
        let z = cz * s + margin + ((r >> 44) % span) as i64;
        let (col, slope) = self.column_and_slope(x, z);
        if col.surface != Surface::Grass
            || slope > 2
            || col.height <= self.sea_level + t.beach_height + 2
        {
            return;
        }
        let mut rng = Rng(r ^ 0x5eed_74ee);
        let height = 56.0 + rng.unit() * 48.0;
        let hue = self.autumn.get(scaled(centre, 600.0)) as f32 + rng.signed() * 0.2;
        let leaves = if hue > 0.3 {
            p.leaves[1]
        } else if hue < -0.4 {
            p.leaves[2]
        } else {
            p.leaves[0]
        };
        let seed = r as u32;
        let origin = I64Vec3::new(x, i64::from(col.height) - 2, z);
        let lean = Vec3::new(rng.signed(), 0.0, rng.signed()) * height * 0.12;
        let r0 = 2.2 + rng.unit() * 1.4;
        let mid = Vec3::new(lean.x * 0.4, height * 0.55, lean.z * 0.4);
        let top = Vec3::new(lean.x, height, lean.z);
        out.push(Feature::capsule(origin, Vec3::ZERO, mid, r0, p.wood, seed));
        out.push(Feature::capsule(origin, mid, top, r0 * 0.7, p.wood, seed));
        let mut clumps = [(Vec3::ZERO, 0.0f32); 6];
        let branches = 3 + (rng.next() % 3) as usize;
        let az0 = rng.unit() * TAU;
        for (i, clump) in clumps.iter_mut().enumerate().take(branches) {
            let along = 0.45 + rng.unit() * 0.4;
            let start = if along < 0.55 {
                Vec3::ZERO.lerp(mid, along / 0.55)
            } else {
                mid.lerp(top, (along - 0.55) / 0.45)
            };
            let az = az0 + i as f32 * TAU / branches as f32 + rng.signed() * 0.5;
            let el = 0.35 + rng.unit() * 0.55;
            let dir = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos());
            let end = start + dir * height * (0.26 + rng.unit() * 0.18);
            out.push(Feature::capsule(
                origin,
                start,
                end,
                1.1 + rng.unit() * 0.7,
                p.wood,
                seed,
            ));
            *clump = (end, (height * (0.14 + rng.unit() * 0.06)).clamp(8.0, 20.0));
        }
        clumps[branches] = (
            top + Vec3::Y * height * 0.04,
            (height * 0.22).clamp(10.0, 22.0),
        );
        for &(c, radius) in &clumps[..=branches] {
            out.push(Feature::blob(origin, c, radius, true, leaves, seed));
            // A smaller clump beside it makes the foliage lumpy.
            let off = Vec3::new(rng.signed(), rng.signed() * 0.5, rng.signed()) * radius * 0.6;
            out.push(Feature::blob(
                origin,
                c + off,
                radius * 0.65,
                true,
                leaves,
                seed ^ 0x9e37,
            ));
        }
    }

    /// The boulder of cell `(cx, cz)`, if it has one: a half-buried noisy blob.
    fn boulder(&self, cx: i64, cz: i64, out: &mut Vec<Feature>) {
        let t = &self.config;
        let p = &self.palette;
        let r = hash2(self.seed ^ SALT_BOULDER, cx, cz);
        if unit(r) >= t.boulder_density {
            return;
        }
        let margin = 16;
        let span = (BOULDER_CELL - 2 * margin) as u64;
        let x = cx * BOULDER_CELL + margin + ((r >> 24) % span) as i64;
        let z = cz * BOULDER_CELL + margin + ((r >> 44) % span) as i64;
        let col = self.column_at(x, z);
        if col.surface == Surface::Sand {
            return;
        }
        let mut rng = Rng(r ^ 0xb01d_e125);
        let radius = if rng.unit() < 0.1 {
            9.0 + rng.unit() * 7.0
        } else {
            3.0 + rng.unit() * 6.0
        };
        let material = if rng.unit() < 0.4 {
            p.sandstone
        } else {
            p.stone
        };
        let origin = I64Vec3::new(x, i64::from(col.height), z);
        let c = Vec3::new(0.0, -radius * 0.35, 0.0);
        out.push(Feature::blob(origin, c, radius, false, material, r as u32));
    }

    /// Material of the features at `w`, if any: wood and rock win over foliage.
    fn feature_material(
        &self,
        w: I64Vec3,
        features: &[Feature],
        indices: &[u16],
    ) -> Option<MaterialId> {
        let mut foliage = None;
        for &i in indices {
            let f = &features[usize::from(i)];
            if f.contains(w) {
                if !f.leafy() {
                    return Some(f.material);
                }
                foliage.get_or_insert(f.material);
            }
        }
        foliage
    }

    /// Generates the chunk at `pos`.
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let _span = tracing::trace_span!("terrain_chunk").entered();
        let column = self.column(IVec2::new(pos.0.x, pos.0.z));
        let cy0 = pos.0.y * CHUNK_SIZE as i32;
        let cy1 = cy0 + CHUNK_SIZE as i32;
        let top = column.brick_top.iter().copied().max().unwrap_or(0);
        if let Some(m) = self.classify_span(column.min, top, cy0, cy1, false)
            && (m == self.palette.stone || cy0 >= column.max + FEATURE_ABOVE)
        {
            return Chunk::uniform(m);
        }
        let origin = pos.origin().0;
        let mut features = Vec::new();
        if cy0 < column.max + FEATURE_ABOVE && cy1 > column.min - FEATURE_BELOW {
            self.features(origin, origin + (CHUNK_SIZE as i64 - 1), &mut features);
        }
        if features.is_empty()
            && let Some(m) = self.classify_span(column.min, top, cy0, cy1, false)
        {
            return Chunk::uniform(m);
        }

        let mut scratch = [MaterialId::AIR; BRICK_VOLUME];
        let mut in_brick: Vec<u16> = Vec::with_capacity(features.len());
        let bricks = Box::new(std::array::from_fn(|brick| {
            let bx = brick % BRICKS_PER_CHUNK_AXIS;
            let by = (brick / BRICKS_PER_CHUNK_AXIS) % BRICKS_PER_CHUNK_AXIS;
            let bz = brick / (BRICKS_PER_CHUNK_AXIS * BRICKS_PER_CHUNK_AXIS);
            let bcol = bx + bz * BRICKS_PER_CHUNK_AXIS;
            let y0 = cy0 + (by * BRICK_SIZE) as i32;
            let bmin = origin + I64Vec3::new(bx as i64, by as i64, bz as i64) * BRICK_SIZE as i64;
            let bmax = bmin + (BRICK_SIZE as i64 - 1);
            in_brick.clear();
            in_brick.extend(
                features
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.overlaps(bmin, bmax))
                    .map(|(i, _)| i as u16),
            );
            if let Some(m) = self.classify_span(
                column.brick_min[bcol],
                column.brick_top[bcol],
                y0,
                y0 + BRICK_SIZE as i32,
                !in_brick.is_empty(),
            ) {
                return Brick::Uniform(m);
            }
            for (local, voxel) in scratch.iter_mut().enumerate() {
                let p = LocalPos::from_brick(brick, local);
                let c = column.column(p.x.into(), p.z.into());
                let mut m = self.column_material(c, cy0 + i32::from(p.y));
                if !in_brick.is_empty() && self.palette.replaceable(m) {
                    let w = origin + p.as_ivec3().as_i64vec3();
                    if let Some(f) = self.feature_material(w, &features, &in_brick) {
                        m = f;
                    }
                }
                *voxel = m;
            }
            Brick::from_voxels(&scratch)
        }));
        Chunk::from_bricks(bricks)
    }

    /// Material at `w`, evaluated on its own with the given features (tests and
    /// tools; the chunk generator is the fast path).
    pub fn material_at(&self, w: WorldPos, features: &[Feature]) -> MaterialId {
        let c = self.column_at(w.0.x, w.0.z);
        let m = self.column_material(&c, w.0.y as i32);
        if !self.palette.replaceable(m) {
            return m;
        }
        let all: Vec<u16> = (0..features.len() as u16).collect();
        self.feature_material(w.0, features, &all).unwrap_or(m)
    }
}

/// A tree part or boulder: a capsule (trunk, branch) or a noisy blob (foliage
/// clump, rock), positioned relative to `origin`.
#[derive(Debug, Clone, Copy)]
pub struct Feature {
    origin: I64Vec3,
    shape: Shape,
    material: MaterialId,
    seed: u32,
    /// World bounding box, inclusive.
    min: I64Vec3,
    max: I64Vec3,
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Capsule {
        a: Vec3,
        b: Vec3,
        r: f32,
    },
    /// Foliage (`leafy`) has a ragged edge and holes; rock is denser and rounder.
    Blob {
        c: Vec3,
        r: f32,
        leafy: bool,
    },
}

impl Feature {
    fn capsule(origin: I64Vec3, a: Vec3, b: Vec3, r: f32, material: MaterialId, seed: u32) -> Self {
        let lo = (a.min(b) - r).floor().as_i64vec3();
        let hi = (a.max(b) + r).ceil().as_i64vec3();
        Self {
            origin,
            shape: Shape::Capsule { a, b, r },
            material,
            seed,
            min: origin + lo,
            max: origin + hi,
        }
    }

    fn blob(
        origin: I64Vec3,
        c: Vec3,
        r: f32,
        leafy: bool,
        material: MaterialId,
        seed: u32,
    ) -> Self {
        let e = r * 1.2;
        Self {
            origin,
            shape: Shape::Blob { c, r, leafy },
            material,
            seed,
            min: origin + (c - e).floor().as_i64vec3(),
            max: origin + (c + e).ceil().as_i64vec3(),
        }
    }

    fn leafy(&self) -> bool {
        matches!(self.shape, Shape::Blob { leafy: true, .. })
    }

    pub fn material(&self) -> MaterialId {
        self.material
    }

    fn overlaps(&self, min: I64Vec3, max: I64Vec3) -> bool {
        self.min.cmple(max).all() && self.max.cmpge(min).all()
    }

    /// Whether the voxel `w` is part of this feature.
    fn contains(&self, w: I64Vec3) -> bool {
        if w.cmplt(self.min).any() || w.cmpgt(self.max).any() {
            return false;
        }
        let p = (w - self.origin).as_vec3() + Vec3::splat(0.5);
        match self.shape {
            Shape::Capsule { a, b, r } => {
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                (p - (a + ab * t)).length_squared() <= r * r
            }
            Shape::Blob { c, r, leafy } => {
                let d2 = (p - c).length_squared();
                let outer = r * 1.15;
                if d2 > outer * outer {
                    return false;
                }
                let (scale, base, spread) = if leafy {
                    (5.0, 0.72, 0.43)
                } else {
                    (3.5, 0.8, 0.35)
                };
                let n = value_noise3(self.seed, p / scale);
                if d2.sqrt() >= r * (base + spread * n) {
                    return false;
                }
                // Foliage has small holes the light gets through.
                !leafy || unit(hash3(self.seed, w)) >= 0.18
            }
        }
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn scaled(p: [f64; 2], wavelength: f64) -> [f64; 2] {
    [p[0] / wavelength, p[1] / wavelength]
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash2(seed: u64, x: i64, z: i64) -> u64 {
    splitmix(seed ^ splitmix(x as u64 ^ splitmix(z as u64)))
}

fn hash3(seed: u32, w: I64Vec3) -> u64 {
    splitmix(u64::from(seed) ^ splitmix(w.x as u64 ^ splitmix(w.y as u64 ^ splitmix(w.z as u64))))
}

/// Low 24 bits of `r` as a number in `[0, 1)`.
fn unit(r: u64) -> f64 {
    (r & 0xff_ffff) as f64 / 16_777_216.0
}

/// Smooth value noise in `[0, 1)`: hashed lattice values blended trilinearly.
fn value_noise3(seed: u32, p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let i = i.as_i64vec3();
    let v = |dx: i64, dy: i64, dz: i64| unit(hash3(seed, i + I64Vec3::new(dx, dy, dz))) as f32;
    let x00 = v(0, 0, 0) + (v(1, 0, 0) - v(0, 0, 0)) * u.x;
    let x10 = v(0, 1, 0) + (v(1, 1, 0) - v(0, 1, 0)) * u.x;
    let x01 = v(0, 0, 1) + (v(1, 0, 1) - v(0, 0, 1)) * u.x;
    let x11 = v(0, 1, 1) + (v(1, 1, 1) - v(0, 1, 1)) * u.x;
    let y0 = x00 + (x10 - x00) * u.y;
    let y1 = x01 + (x11 - x01) * u.y;
    y0 + (y1 - y0) * u.z
}

/// Small deterministic random stream for building one feature.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix(self.0)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / 16_777_216.0
    }

    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> TerrainPalette {
        TerrainPalette::synthetic()
    }

    fn generator(seed: u64) -> TerrainGenerator {
        TerrainGenerator::new(&WorldConfig::default(), seed, palette())
    }

    fn legacy(seed: u64) -> TerrainGenerator {
        let mut config = WorldConfig::default();
        config.terrain.generator = 1;
        TerrainGenerator::new(&config, seed, palette())
    }

    /// Reference implementation: evaluates every voxel independently.
    fn reference(g: &TerrainGenerator, pos: ChunkPos) -> Chunk {
        let origin = pos.origin().0;
        let mut features = Vec::new();
        g.features(origin, origin + 63, &mut features);
        let all: Vec<u16> = (0..features.len() as u16).collect();
        // Columns on their own (no height grid), once per column to keep it fast.
        let columns: Vec<Column> = (0..COLUMN_AREA as i64)
            .map(|i| g.column_at(origin.x + i % 64, origin.z + i / 64))
            .collect();
        Chunk::from_fn(pos, |w: WorldPos| {
            let c = &columns[((w.0.x - origin.x) + (w.0.z - origin.z) * 64) as usize];
            let m = g.column_material(c, w.0.y as i32);
            if g.palette.replaceable(m) {
                g.feature_material(w.0, &features, &all).unwrap_or(m)
            } else {
                m
            }
        })
    }

    fn check(g: &TerrainGenerator, pos: ChunkPos) {
        assert_eq!(
            g.generate(pos).data(),
            reference(g, pos).data(),
            "mismatch at {pos:?}"
        );
    }

    /// A tree base in the first forest found (searching cells outwards).
    fn find_tree(g: &TerrainGenerator) -> I64Vec3 {
        let mut features = Vec::new();
        for i in 0..64i64 {
            let c = I64Vec3::new(i * 512, 0, -i * 384);
            features.clear();
            g.features(
                c - I64Vec3::new(256, 4096, 256),
                c + I64Vec3::new(256, 4096, 256),
                &mut features,
            );
            if let Some(f) = features.iter().find(|f| f.material == g.palette.wood) {
                return f.origin;
            }
        }
        panic!("no tree found");
    }

    #[test]
    fn fast_paths_match_reference() {
        for g in [generator(7), legacy(7)] {
            // Find a column, then test every chunk from deep stone to open air.
            let h = g.height_at(0, 0);
            let surface = h.div_euclid(CHUNK_SIZE as i32);
            for cx in [-1, 0] {
                for cy in (surface - 2)..=(surface + 2) {
                    check(&g, ChunkPos::new(cx, cy, 0));
                }
            }
            // Sea-level chunks too (water/air interfaces).
            for cx in -3..3 {
                check(&g, ChunkPos::new(cx * 7, 0, cx * 5));
                check(&g, ChunkPos::new(cx * 7, -1, cx * 5));
            }
        }
        // Chunks a tree crosses.
        let g = generator(7);
        let base = WorldPos(find_tree(&g)).chunk();
        for dy in 0..=2 {
            check(&g, ChunkPos::new(base.0.x, base.0.y + dy, base.0.z));
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
        let grass = Column {
            height: 100, // well above the beach
            ..Column::default()
        };
        assert_eq!(g.column_material(&grass, 100), MaterialId::AIR);
        assert_eq!(g.column_material(&grass, 99), p.grass);
        assert_eq!(g.column_material(&grass, 100 - t.grass_depth - 1), p.dirt);
        assert_eq!(g.column_material(&grass, 100 - t.dirt_depth - 1), p.stone);
        // A tuft with a flower on top.
        let tuft = Column {
            decor: 4,
            flower: 2,
            ..grass
        };
        assert_eq!(g.column_material(&tuft, 100), p.tall_grass);
        assert_eq!(g.column_material(&tuft, 103), p.flowers[1]);
        assert_eq!(g.column_material(&tuft, 104), MaterialId::AIR);
        // Bare rock: strata down to the dirt depth, stone below.
        let rock = Column {
            surface: Surface::Rock,
            ..grass
        };
        assert!([p.sandstone, p.stone, p.basalt].contains(&g.column_material(&rock, 99)));
        assert_eq!(g.column_material(&rock, 100 - t.dirt_depth - 1), p.stone);
        // Under water: sand floor, water above it up to the sea level.
        let sea = Column {
            height: -20,
            surface: Surface::Sand,
            ..Column::default()
        };
        assert_eq!(g.column_material(&sea, -21), p.sand);
        assert_eq!(g.column_material(&sea, -20), p.water);
        assert_eq!(g.column_material(&sea, -1), p.water);
        assert_eq!(g.column_material(&sea, 0), MaterialId::AIR);
    }

    #[test]
    fn natural_landscape_has_vegetation_trees_boulders_and_rock() {
        let g = generator(5);
        let (mut tufts, mut flowers, mut rock) = (0, 0, 0);
        for z in (-8192..8192).step_by(61) {
            for x in (-8192..8192).step_by(47) {
                let c = g.column_at(x, z);
                tufts += usize::from(c.decor > 0);
                flowers += usize::from(c.flower > 0);
                rock += usize::from(c.surface == Surface::Rock);
                if c.decor > 0 {
                    assert!(c.height > g.sea_level(), "no vegetation under water");
                }
            }
        }
        assert!(
            tufts > 1000 && flowers > 50 && rock > 10,
            "{tufts} tufts, {flowers} flowers, {rock} rock"
        );
        let base = find_tree(&g);
        // The trunk stands on the ground, with foliage above it.
        let mut features = Vec::new();
        g.features(base - 128, base + 160, &mut features);
        let trunk = WorldPos(base + I64Vec3::new(0, 4, 0));
        assert_eq!(g.material_at(trunk, &features), g.palette.wood);
        assert!(
            features
                .iter()
                .any(|f| g.palette.leaves.contains(&f.material))
        );
        let mut boulders = Vec::new();
        g.features(I64Vec3::splat(-2048), I64Vec3::splat(2048), &mut boulders);
        assert!(
            boulders
                .iter()
                .any(|f| matches!(f.shape, Shape::Blob { leafy: false, .. }))
        );
    }

    #[test]
    fn generator_1_is_the_plain_heightmap() {
        let g = legacy(5);
        let mut config = WorldConfig::default();
        config.terrain.generator = 1;
        config.terrain.roughness_amplitude = 0.0;
        config.terrain.cliff_amplitude = 0.0;
        let plain = TerrainGenerator::new(&config, 5, palette());
        let mut features = Vec::new();
        g.features(I64Vec3::splat(-4096), I64Vec3::splat(4096), &mut features);
        assert!(features.is_empty());
        for i in -200..200 {
            let (x, z) = (i * 37, i * -53);
            assert_eq!(g.height_at(x, z), plain.height_at(x, z));
            let c = g.column_at(x, z);
            assert_eq!((c.decor, c.surface == Surface::Rock), (0, false));
        }
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
