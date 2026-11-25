//! Temperature on a coarse grid (PROMPT.md §6.3, ADR 007) and combustion (ADR 008).
//!
//! Each chunk with a temperature field holds 16³ cells of 4³ voxels. A cell stores
//! its temperature plus the heat capacity (sum of `density × heat_capacity` of its
//! voxels), mean conductivity and heat loss of its contents. A 4³ cell always lies
//! inside one 8³ brick, so uniform bricks give the composition of 8 cells at once.
//!
//! Chunks without a field are at the ambient temperature.

use glam::IVec3;
use mv_core::LocalPos;
use mv_core::consts::{
    BRICK_SIZE, BRICKS_PER_CHUNK, CHUNK_SIZE, THERMAL_CELL_SIZE, THERMAL_CELLS_PER_AXIS,
    THERMAL_CELLS_PER_CHUNK,
};
use mv_world::{BrickRef, Chunk, MaterialId};

use crate::aabb::Aabb;
use crate::rng;
use crate::table::{Kind, MaterialTable, ThermalProps};

/// Cell edge in voxels.
pub const CELL: i32 = THERMAL_CELL_SIZE as i32;
/// Cells along a chunk axis.
pub const CELLS_AXIS: i32 = THERMAL_CELLS_PER_AXIS as i32;
/// Cells per chunk.
pub const CELLS: usize = THERMAL_CELLS_PER_CHUNK;

pub type CellArray = [f32; CELLS];

// `cell_of` divides by shifting.
const _: () = assert!(CELL == 4);

/// Salts that decorrelate the random rolls of the thermal pass.
const SALT_PHASE: u64 = 0x7E3A_11C5;
const SALT_BURN: u64 = 0xB0B1_F12E;
const SALT_FLAME: u64 = 0x0F1A_3E55;

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
    /// Heat lost to the environment per tick and kelvin above ambient.
    pub loss: Box<CellArray>,
    /// Ticks the whole field has stayed within epsilon of ambient.
    pub idle_ticks: u32,
}

const PER_CELL: f32 = (CELL * CELL * CELL) as f32;

impl ThermalChunk {
    /// A field at `temperature` with the composition of `chunk`.
    pub fn new(chunk: &Chunk, table: &MaterialTable, temperature: f32) -> Self {
        let mut t = Self {
            temp: Box::new([temperature; CELLS]),
            cap: Box::new([0.0; CELLS]),
            cond: Box::new([0.0; CELLS]),
            loss: Box::new([0.0; CELLS]),
            idle_ticks: 0,
        };
        t.recompute(chunk, table, Aabb::CHUNK);
        t
    }

    /// Recomputes capacity, conductivity and loss of the cells overlapping `region`
    /// (chunk-local voxels), keeping their temperatures.
    pub fn recompute(&mut self, chunk: &Chunk, table: &MaterialTable, region: Aabb) {
        let region = region.intersect(Aabb::CHUNK);
        if region.is_empty() {
            return;
        }
        let bmin = region.min / BRICK_SIZE as i32;
        let bmax = region.max / BRICK_SIZE as i32;
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
                                self.cap[c] = p.capacity * PER_CELL;
                                self.cond[c] = p.conductivity;
                                self.loss[c] = p.loss * PER_CELL;
                            }
                        }
                        BrickRef::Dense(voxels) => {
                            for c in cells {
                                self.cap[c] = 0.0;
                                self.cond[c] = 0.0;
                                self.loss[c] = 0.0;
                            }
                            for (local, &m) in voxels.iter().enumerate() {
                                let lp = LocalPos::from_brick(b, local).as_ivec3();
                                let c = cell_of(lp);
                                let p = table.thermal(m);
                                self.cap[c] += p.capacity;
                                self.cond[c] += p.conductivity / PER_CELL;
                                self.loss[c] += p.loss;
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

    /// Sensible energy of one cell.
    #[inline]
    pub fn energy_of(&self, cell: usize) -> f64 {
        f64::from(self.temp[cell]) * f64::from(self.cap[cell])
    }

    /// Updates the composition sums of `cell` for a voxel `old → new`.
    #[inline]
    fn swap_composition(&mut self, cell: usize, old: &ThermalProps, new: &ThermalProps) -> f32 {
        let new_cap = (self.cap[cell] - old.capacity + new.capacity).max(1e-3);
        self.cap[cell] = new_cap;
        self.cond[cell] =
            (self.cond[cell] + (new.conductivity - old.conductivity) / PER_CELL).max(0.0);
        self.loss[cell] = (self.loss[cell] + new.loss - old.loss).max(0.0);
        new_cap
    }

    /// Replaces a voxel `old` (at the cell's temperature) by a voxel `new` arriving at
    /// `t_in`. Conserves energy within the cell.
    #[inline]
    pub fn replace_voxel(
        &mut self,
        cell: usize,
        old: &ThermalProps,
        new: &ThermalProps,
        t_in: f32,
    ) {
        let t = self.temp[cell];
        let energy = t * self.cap[cell] - old.capacity * t + new.capacity * t_in;
        let new_cap = self.swap_composition(cell, old, new);
        self.temp[cell] = energy / new_cap;
    }

    /// A voxel `pa` moved from cell `a` to `b` and a voxel `pb` the other way: each
    /// carries its energy (advection).
    #[inline]
    pub fn advect(&mut self, a: usize, b: usize, pa: &ThermalProps, pb: &ThermalProps) {
        if a == b {
            return;
        }
        let (ta, tb) = (self.temp[a], self.temp[b]);
        self.replace_voxel(a, pa, pb, tb);
        self.replace_voxel(b, pb, pa, ta);
    }

    /// A voxel of `cell` changes material in place (phase change, reaction, decay,
    /// combustion). The cell keeps its temperature except for the latent heat the
    /// voxel absorbs (`new.latent − old.latent`) or releases; released heat never
    /// lifts the cell above `max(T, max_temp)`. Returns the energy not conserved
    /// (ADR 007): sensible change plus latent change.
    #[inline]
    pub fn change_voxel(
        &mut self,
        cell: usize,
        old: &ThermalProps,
        new: &ThermalProps,
        max_temp: f32,
    ) -> f64 {
        let t = self.temp[cell];
        let before = f64::from(t) * f64::from(self.cap[cell]);
        let latent = new.latent - old.latent;
        let new_cap = self.swap_composition(cell, old, new);
        let mut t_new = t - latent / new_cap;
        if latent < 0.0 {
            t_new = t_new.min(max_temp.max(t));
        }
        self.temp[cell] = t_new;
        let after = f64::from(t_new) * f64::from(new_cap);
        (after - before) + f64::from(latent)
    }

    /// Adds `energy` to `cell`; positive energy never lifts the cell above
    /// `max(T, cap_temp)`. Returns the energy actually added.
    #[inline]
    pub fn add_heat(&mut self, cell: usize, energy: f32, cap_temp: f32) -> f32 {
        let (t, cap) = (self.temp[cell], self.cap[cell].max(1e-3));
        let mut t_new = t + energy / cap;
        if energy > 0.0 {
            t_new = t_new.min(cap_temp.max(t));
        }
        self.temp[cell] = t_new;
        (t_new - t) * cap
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

/// Energy change of a voxel `old → new` in a chunk without a field (held at
/// `ambient`): what [`ThermalChunk::change_voxel`] returns in that case.
#[inline]
pub fn ambient_change(old: &ThermalProps, new: &ThermalProps, ambient: f32) -> f64 {
    f64::from(ambient) * f64::from(new.capacity - old.capacity) + f64::from(new.latent - old.latent)
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
    pub ambient: f32,
}

/// One explicit (Jacobi) diffusion step of `me` into `out`. `neighbors` are the
/// fields of the 6 face neighbours in the order −x, +x, −y, +y, −z, +z (missing
/// ones are insulating). Face fluxes are antisymmetric and limited by the smaller
/// capacity of the two cells, so energy is conserved and the step is stable. Each
/// cell also loses `loss × (T − ambient)` to the environment; returns that total.
pub fn diffuse(
    me: &ThermalChunk,
    neighbors: [Option<&ThermalChunk>; 6],
    params: DiffusionParams,
    out: &mut CellArray,
) -> f64 {
    let n = CELLS_AXIS;
    const DIRS: [IVec3; 6] = [
        IVec3::NEG_X,
        IVec3::X,
        IVec3::NEG_Y,
        IVec3::Y,
        IVec3::NEG_Z,
        IVec3::Z,
    ];
    let mut lost = 0.0f64;
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
        // Faces use at most 0.9 of the capacity; the loss at most 0.1 (stable).
        let h = me.loss[i].min(0.1 * cap);
        if h > 0.0 {
            let q = h * (t - params.ambient);
            d_energy -= q;
            lost += f64::from(q);
        }
        out[i] = t + d_energy / cap;
    }
    lost
}

/// Parameters of the transition pass.
#[derive(Debug, Clone, Copy)]
pub struct TransitionParams {
    pub hysteresis: f32,
    /// Chance per tick that an eligible voxel changes phase.
    pub rate: u32,
    pub seed: u64,
    pub tick: u64,
}

/// Result of the transition pass over one chunk.
#[derive(Debug, Default)]
pub struct TransitionOut {
    pub changed: Aabb,
    /// Phase changes and combustions.
    pub count: u32,
    /// Flames emitted.
    pub flames: u32,
    /// Energy not conserved by the volume-preserving phase changes (ADR 007):
    /// (sensible after − sensible before) + latent absorbed.
    pub imbalance: f64,
    /// Cells where a solid disappeared (melted, burnt).
    pub solid_lost: Vec<IVec3>,
}

/// Directions a flame may be emitted in: up first, then the sides.
const FLAME_DIRS: [IVec3; 5] = [IVec3::Y, IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z];

/// Applies temperature-driven changes (melt, boil, freeze, burn) to the voxels of
/// `chunk`, and lets burning voxels emit flames into adjacent air of the same chunk.
pub fn transitions(
    chunk: &mut Chunk,
    field: &mut ThermalChunk,
    origin: glam::I64Vec3,
    table: &MaterialTable,
    params: TransitionParams,
) -> TransitionOut {
    let _span = tracing::trace_span!("transitions_chunk").entered();
    let mut out = TransitionOut::default();
    for b in 0..BRICKS_PER_CHUNK {
        let origin_b = LocalPos::from_brick(b, 0).as_ivec3();
        let cells = brick_cells(origin_b);
        // Skip the brick if nothing in it can change at its cells' temperatures.
        if let BrickRef::Uniform(m) = chunk.brick(b) {
            let p = table.thermal(m);
            if !p.has_changes()
                || cells
                    .iter()
                    .all(|&c| !p.active_at(field.temp[c], params.hysteresis))
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
            let t = field.temp[c];
            let world = origin + pi.as_i64vec3();
            if let Some(into) = p.change_at(t, params.hysteresis) {
                let rate = p.rate.unwrap_or(params.rate);
                if rng::hash(params.seed ^ SALT_PHASE, params.tick, world) < rate {
                    if table.get(m).kind == Kind::Static && table.get(into).kind != Kind::Static {
                        out.solid_lost.push(pi);
                    }
                    out.imbalance += field.change_voxel(c, p, table.thermal(into), f32::INFINITY);
                    chunk.set(lp, into);
                    out.changed.include(pi);
                    out.count += 1;
                }
                continue;
            }
            let Some(burn) = p.burning_at(t).copied() else {
                continue;
            };
            if rng::hash(params.seed ^ SALT_BURN, params.tick, world) < burn.rate {
                if table.get(m).kind == Kind::Static && table.get(burn.into).kind != Kind::Static {
                    out.solid_lost.push(pi);
                }
                out.imbalance += field.change_voxel(c, p, table.thermal(burn.into), burn.max_temp);
                chunk.set(lp, burn.into);
                out.changed.include(pi);
                out.count += 1;
                continue;
            }
            let Some(flame) = burn.flame else {
                continue;
            };
            let r = rng::hash(params.seed ^ SALT_FLAME, params.tick, world);
            if r >= burn.flame_rate {
                continue;
            }
            // Up if possible, else a side picked from the roll.
            let side = 1 + (r >> 8) as usize % 4;
            for d in [FLAME_DIRS[0], FLAME_DIRS[side]] {
                let q = pi + d;
                if (q.cmplt(IVec3::splat(CHUNK_SIZE as i32))).all() && q.cmpge(IVec3::ZERO).all() {
                    let lq = LocalPos::new(q.x as u8, q.y as u8, q.z as u8);
                    if chunk.get(lq).is_air() {
                        let cq = cell_of(q);
                        out.imbalance += field.change_voxel(
                            cq,
                            table.thermal(MaterialId::AIR),
                            table.thermal(flame),
                            f32::INFINITY,
                        );
                        chunk.set(lq, flame);
                        out.changed.include(q);
                        out.flames += 1;
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Whether `m` would change at temperature `t` (used to decide if a material
/// placed or moved into a chunk without a field needs one).
pub fn unstable_at(table: &MaterialTable, m: MaterialId, t: f32, hysteresis: f32) -> bool {
    table.thermal(m).active_at(t, hysteresis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(capacity: f32) -> ThermalProps {
        ThermalProps {
            capacity,
            conductivity: 1.0,
            loss: 0.0,
            latent: 0.0,
            base_temperature: None,
            rate: None,
            up: [None; 2],
            down: None,
            burn: None,
        }
    }

    fn field() -> ThermalChunk {
        ThermalChunk {
            temp: Box::new([20.0; CELLS]),
            cap: Box::new([100.0; CELLS]),
            cond: Box::new([1.0; CELLS]),
            loss: Box::new([0.0; CELLS]),
            idle_ticks: 0,
        }
    }

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
        let mut f = field();
        f.temp[0] = 500.0;
        f.cap[0] = 5000.0;
        let e0 = f.energy();
        f.advect(0, 1, &props(4000.0), &props(1.0));
        assert!((f.energy() - e0).abs() < 1e-3 * e0);
        assert!(f.temp[1] > 400.0, "the dense hot voxel heats its new cell");
        assert!((f.cap[0] - 1001.0).abs() < 1e-3);
    }

    #[test]
    fn released_heat_is_capped_by_the_flame_temperature() {
        let mut f = field();
        let mut fuel = props(50.0);
        fuel.latent = 1e6;
        let ash = props(50.0);
        let e0 = f.energy();
        let imbalance = f.change_voxel(0, &fuel, &ash, 800.0);
        assert!((f.temp[0] - 800.0).abs() < 1e-3, "capped at {}", f.temp[0]);
        // The bookkeeping still balances: sensible gained + latent lost.
        let latent_change = -1e6;
        assert!(((f.energy() - e0) + latent_change - imbalance).abs() < 1.0);
        // Already hotter than the flame: burning does not cool it.
        f.temp[1] = 900.0;
        f.change_voxel(1, &fuel, &ash, 800.0);
        assert!((f.temp[1] - 900.0).abs() < 1e-3);
    }

    #[test]
    fn heat_loss_pulls_towards_ambient() {
        let mut f = field();
        f.temp[5] = 500.0;
        f.loss[5] = 5.0;
        let mut out = [0.0; CELLS];
        let lost = diffuse(
            &f,
            [None; 6],
            DiffusionParams {
                rate: 0.0,
                ambient: 20.0,
            },
            &mut out,
        );
        assert!(out[5] < 500.0 && out[5] > 20.0);
        assert!((lost - 5.0 * 480.0).abs() < 1e-3);
        assert!((out[6] - 20.0).abs() < 1e-6);
    }
}
