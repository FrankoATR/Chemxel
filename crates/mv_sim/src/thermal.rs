//! Temperature on a coarse grid (PROMPT.md §6.3, ADR 007).
//!
//! Each chunk with a temperature field holds 16³ cells of 4³ voxels. A cell stores
//! its temperature plus the heat capacity (sum of `density × heat_capacity` of its
//! voxels) and mean conductivity of its contents. A 4³ cell always lies inside one
//! 8³ brick, so uniform bricks give the composition of 8 cells at once.
//!
//! Chunks without a field are at the ambient temperature.

use glam::IVec3;
use mv_core::LocalPos;
use mv_core::consts::{
    BRICK_SIZE, BRICKS_PER_CHUNK, THERMAL_CELL_SIZE, THERMAL_CELLS_PER_AXIS,
    THERMAL_CELLS_PER_CHUNK,
};
use mv_world::{BrickRef, Chunk, MaterialId};

use crate::aabb::Aabb;
use crate::rng;
use crate::table::MaterialTable;

/// Cell edge in voxels.
pub const CELL: i32 = THERMAL_CELL_SIZE as i32;
/// Cells along a chunk axis.
pub const CELLS_AXIS: i32 = THERMAL_CELLS_PER_AXIS as i32;
/// Cells per chunk.
pub const CELLS: usize = THERMAL_CELLS_PER_CHUNK;

pub type CellArray = [f32; CELLS];

// `cell_of` divides by shifting.
const _: () = assert!(CELL == 4);

/// Cell index of chunk-local voxel `p` (x fastest, like every other index).
#[inline]
pub fn cell_of(p: IVec3) -> usize {
    ((p.x >> 2) + (p.y >> 2) * CELLS_AXIS + (p.z >> 2) * CELLS_AXIS * CELLS_AXIS) as usize
}

#[inline]
fn cell_coords(i: usize) -> IVec3 {
    let i = i as i32;
    IVec3::new(
        i % CELLS_AXIS,
        (i / CELLS_AXIS) % CELLS_AXIS,
        i / (CELLS_AXIS * CELLS_AXIS),
    )
}

/// Temperature field of one chunk.
#[derive(Debug, Clone)]
pub struct ThermalChunk {
    pub temp: Box<CellArray>,
    pub cap: Box<CellArray>,
    pub cond: Box<CellArray>,
    /// Ticks the whole field has stayed within epsilon of ambient.
    pub idle_ticks: u32,
}

impl ThermalChunk {
    /// A field at `temperature` with the composition of `chunk`.
    pub fn new(chunk: &Chunk, table: &MaterialTable, temperature: f32) -> Self {
        let mut t = Self {
            temp: Box::new([temperature; CELLS]),
            cap: Box::new([0.0; CELLS]),
            cond: Box::new([0.0; CELLS]),
            idle_ticks: 0,
        };
        t.recompute(chunk, table, Aabb::CHUNK);
        t
    }

    /// Recomputes capacity and conductivity of the cells overlapping `region`
    /// (chunk-local voxels), keeping their temperatures.
    pub fn recompute(&mut self, chunk: &Chunk, table: &MaterialTable, region: Aabb) {
        let region = region.intersect(Aabb::CHUNK);
        if region.is_empty() {
            return;
        }
        let bmin = region.min / BRICK_SIZE as i32;
        let bmax = region.max / BRICK_SIZE as i32;
        let per_cell = (CELL * CELL * CELL) as f32;
        for bz in bmin.z..=bmax.z {
            for by in bmin.y..=bmax.y {
                for bx in bmin.x..=bmax.x {
                    let b = (bx + by * 8 + bz * 64) as usize;
                    debug_assert!(b < BRICKS_PER_CHUNK);
                    let origin = IVec3::new(bx, by, bz) * BRICK_SIZE as i32;
                    let cells = brick_cells(origin);
                    match chunk.brick(b) {
                        BrickRef::Uniform(m) => {
                            let p = table.thermal(m);
                            for c in cells {
                                self.cap[c] = p.capacity * per_cell;
                                self.cond[c] = p.conductivity;
                            }
                        }
                        BrickRef::Dense(voxels) => {
                            for c in cells {
                                self.cap[c] = 0.0;
                                self.cond[c] = 0.0;
                            }
                            for (local, &m) in voxels.iter().enumerate() {
                                let lp = LocalPos::from_brick(b, local).as_ivec3();
                                let c = cell_of(lp);
                                let p = table.thermal(m);
                                self.cap[c] += p.capacity;
                                self.cond[c] += p.conductivity / per_cell;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Total sensible energy (Σ capacity × temperature).
    pub fn energy(&self) -> f64 {
        self.temp
            .iter()
            .zip(self.cap.iter())
            .map(|(&t, &c)| f64::from(t) * f64::from(c))
            .sum()
    }

    /// Replaces a voxel of capacity `c_out` (at the cell's temperature) by one of
    /// capacity `c_in` arriving at `t_in`. Conserves energy within the cell.
    #[inline]
    pub fn replace_voxel(&mut self, cell: usize, c_out: f32, c_in: f32, t_in: f32) {
        let t = self.temp[cell];
        let cap = self.cap[cell];
        let new_cap = (cap - c_out + c_in).max(1e-3);
        let energy = t * cap - c_out * t + c_in * t_in;
        self.cap[cell] = new_cap;
        self.temp[cell] = energy / new_cap;
    }

    /// A voxel of capacity `c_a` moved from cell `a` to `b` and one of `c_b` the
    /// other way: each carries its energy (advection).
    #[inline]
    pub fn advect(&mut self, a: usize, b: usize, c_a: f32, c_b: f32) {
        if a == b {
            return;
        }
        let (ta, tb) = (self.temp[a], self.temp[b]);
        self.replace_voxel(a, c_a, c_b, tb);
        self.replace_voxel(b, c_b, c_a, ta);
    }

    /// Largest |T − ambient| over the field.
    pub fn max_deviation(&self, ambient: f32) -> f32 {
        self.temp
            .iter()
            .fold(0.0f32, |m, &t| m.max((t - ambient).abs()))
    }

    /// Whether any cell on the face of the chunk towards `dir` (a unit axis vector)
    /// deviates from ambient by more than `epsilon`.
    pub fn face_deviates(&self, dir: IVec3, ambient: f32, epsilon: f32) -> bool {
        let n = CELLS_AXIS;
        let axis = if dir.x != 0 {
            0
        } else if dir.y != 0 {
            1
        } else {
            2
        };
        let fixed = if dir[axis] > 0 { n - 1 } else { 0 };
        for v in 0..n {
            for u in 0..n {
                let mut c = IVec3::ZERO;
                c[axis] = fixed;
                c[(axis + 1) % 3] = u;
                c[(axis + 2) % 3] = v;
                let i = (c.x + c.y * n + c.z * n * n) as usize;
                if (self.temp[i] - ambient).abs() > epsilon {
                    return true;
                }
            }
        }
        false
    }
}

/// The 8 cells of the brick whose minimum corner is `origin` (chunk-local voxels).
fn brick_cells(origin: IVec3) -> [usize; 8] {
    let mut out = [0; 8];
    for (k, slot) in out.iter_mut().enumerate() {
        let d = IVec3::new((k & 1) as i32, ((k >> 1) & 1) as i32, ((k >> 2) & 1) as i32);
        *slot = cell_of(origin + d * CELL);
    }
    out
}

/// Diffusion parameters.
#[derive(Debug, Clone, Copy)]
pub struct DiffusionParams {
    pub rate: f32,
}

/// One explicit (Jacobi) diffusion step of `me` into `out`. `neighbors` are the
/// fields of the 6 face neighbours in the order −x, +x, −y, +y, −z, +z (missing
/// ones are insulating). Face fluxes are antisymmetric and limited by the smaller
/// capacity of the two cells, so energy is conserved and the step is stable.
pub fn diffuse(
    me: &ThermalChunk,
    neighbors: [Option<&ThermalChunk>; 6],
    params: DiffusionParams,
    out: &mut CellArray,
) {
    let n = CELLS_AXIS;
    const DIRS: [IVec3; 6] = [
        IVec3::NEG_X,
        IVec3::X,
        IVec3::NEG_Y,
        IVec3::Y,
        IVec3::NEG_Z,
        IVec3::Z,
    ];
    for i in 0..CELLS {
        let c = cell_coords(i);
        let (t, cap, k) = (me.temp[i], me.cap[i], me.cond[i]);
        let mut d_energy = 0.0;
        for (f, dir) in DIRS.iter().enumerate() {
            let q = c + *dir;
            let (field, j) = if q.cmpge(IVec3::ZERO).all() && q.cmplt(IVec3::splat(n)).all() {
                (me, (q.x + q.y * n + q.z * n * n) as usize)
            } else {
                let Some(nb) = neighbors[f] else {
                    continue;
                };
                let w = q.rem_euclid(IVec3::splat(n));
                (nb, (w.x + w.y * n + w.z * n * n) as usize)
            };
            let (tn, capn, kn) = (field.temp[j], field.cap[j], field.cond[j]);
            let ksum = k + kn;
            if ksum <= 0.0 {
                continue;
            }
            let g = (params.rate * 2.0 * k * kn / ksum).min(0.9 * cap.min(capn) / 6.0);
            d_energy += g * (tn - t);
        }
        out[i] = t + d_energy / cap;
    }
}

/// Parameters of the transition pass.
#[derive(Debug, Clone, Copy)]
pub struct TransitionParams {
    pub hysteresis: f32,
    /// Probability per tick that an eligible voxel changes.
    pub rate: f32,
    pub seed: u64,
    pub tick: u64,
}

/// Result of the transition pass over one chunk.
#[derive(Debug, Default)]
pub struct TransitionOut {
    pub changed: Aabb,
    pub count: u32,
    /// Energy not conserved by the volume-preserving phase changes (ADR 007):
    /// (sensible after − sensible before) + latent absorbed.
    pub imbalance: f64,
}

/// Applies temperature-driven changes (melt, boil, freeze...) to the voxels of
/// `chunk`. A changing voxel keeps its cell's temperature except for the latent
/// heat it absorbs or releases.
pub fn transitions(
    chunk: &mut Chunk,
    field: &mut ThermalChunk,
    origin: glam::I64Vec3,
    table: &MaterialTable,
    params: TransitionParams,
) -> TransitionOut {
    let _span = tracing::trace_span!("transitions_chunk").entered();
    let mut out = TransitionOut::default();
    let threshold = (params.rate.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
    for b in 0..BRICKS_PER_CHUNK {
        let origin_b = LocalPos::from_brick(b, 0).as_ivec3();
        let cells = brick_cells(origin_b);
        // Skip the brick if nothing in it can change at its cells' temperatures.
        if let BrickRef::Uniform(m) = chunk.brick(b) {
            let p = table.thermal(m);
            if !p.has_changes()
                || cells
                    .iter()
                    .all(|&c| p.change_at(field.temp[c], params.hysteresis).is_none())
            {
                continue;
            }
        }
        for local in 0..mv_core::consts::BRICK_VOLUME {
            let lp = LocalPos::from_brick(b, local);
            let m = chunk.get(lp);
            let p = table.thermal(m);
            if !p.has_changes() {
                continue;
            }
            let pi = lp.as_ivec3();
            let c = cell_of(pi);
            let Some(into) = p.change_at(field.temp[c], params.hysteresis) else {
                continue;
            };
            let world = origin + pi.as_i64vec3();
            if rng::hash(params.seed ^ 0x7E3A_11C5, params.tick, world) > threshold {
                continue;
            }
            let q = table.thermal(into);
            let before = f64::from(field.temp[c]) * f64::from(field.cap[c]);
            let latent = q.latent - p.latent;
            let new_cap = (field.cap[c] - p.capacity + q.capacity).max(1e-3);
            field.temp[c] -= latent / new_cap;
            field.cap[c] = new_cap;
            let after = f64::from(field.temp[c]) * f64::from(new_cap);
            out.imbalance += (after - before) + f64::from(latent);
            chunk.set(lp, into);
            out.changed.include(pi);
            out.count += 1;
        }
    }
    out
}

/// Whether `m` would change at temperature `t` (used to decide if a material
/// placed or moved into a chunk without a field needs one).
pub fn unstable_at(table: &MaterialTable, m: MaterialId, t: f32, hysteresis: f32) -> bool {
    table.thermal(m).change_at(t, hysteresis).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_index_and_brick_cells() {
        assert_eq!(cell_of(IVec3::new(3, 3, 3)), 0);
        assert_eq!(cell_of(IVec3::new(4, 0, 0)), 1);
        assert_eq!(cell_of(IVec3::new(0, 4, 0)), 16);
        assert_eq!(cell_of(IVec3::new(63, 63, 63)), CELLS - 1);
        let cells = brick_cells(IVec3::new(8, 0, 0));
        assert_eq!(cells[0], 2);
        assert_eq!(cells[1], 3);
        assert_eq!(cells[2], 18);
        assert_eq!(
            cell_coords(cell_of(IVec3::new(40, 20, 60))),
            IVec3::new(10, 5, 15)
        );
    }

    #[test]
    fn advection_conserves_energy() {
        let mut f = ThermalChunk {
            temp: Box::new([20.0; CELLS]),
            cap: Box::new([100.0; CELLS]),
            cond: Box::new([1.0; CELLS]),
            idle_ticks: 0,
        };
        f.temp[0] = 500.0;
        f.cap[0] = 5000.0;
        let e0 = f.energy();
        f.advect(0, 1, 4000.0, 1.0);
        assert!((f.energy() - e0).abs() < 1e-3 * e0);
        assert!(f.temp[1] > 400.0, "the dense hot voxel heats its new cell");
        assert!((f.cap[0] - 1001.0).abs() < 1e-3);
    }
}
