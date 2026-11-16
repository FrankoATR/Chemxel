//! Cellular automaton rules for one chunk (PROMPT.md §6.2).
//!
//! A task owns its chunk mutably and sees its 26 neighbours read-only (they are never
//! processed in the same checkerboard phase). Moves that stay inside the chunk are
//! applied immediately; moves into a neighbour are queued as [`Move`]s and applied
//! serially after the phase (see `Simulation::step`).
//!
//! Rules by [`Kind`]:
//! * **Powder:** fall; else slide to one of the 4 lower diagonals (random order).
//! * **Liquid:** fall; else lower diagonals; else flow sideways up to `dispersion`
//!   cells in one of the 4 horizontal directions (random order).
//! * **Gas:** rise; else upper diagonals; else drift one cell sideways.
//!
//! "Moving" is always a swap with the target: a denser material sinks through a
//! lighter fluid (sand through water, water through air) and the fluid takes its
//! place. Unloaded neighbours act as walls.

use glam::{I64Vec3, IVec3};
use mv_core::consts::CHUNK_SIZE;
use mv_core::{LocalPos, WorldPos};
use mv_world::{BrickRef, Chunk, MaterialId};

use crate::aabb::Aabb;
use crate::rng;
use crate::table::{Kind, MaterialTable, Props};

const N: i32 = CHUNK_SIZE as i32;
/// Words of the per-chunk "moved this tick" bitset (one bit per voxel).
pub const MOVED_WORDS: usize = mv_core::consts::CHUNK_VOLUME / 64;
pub type MovedBits = [u64; MOVED_WORDS];

const DOWN_DIAGONALS: [IVec3; 4] = [
    IVec3::new(1, -1, 0),
    IVec3::new(-1, -1, 0),
    IVec3::new(0, -1, 1),
    IVec3::new(0, -1, -1),
];
const UP_DIAGONALS: [IVec3; 4] = [
    IVec3::new(1, 1, 0),
    IVec3::new(-1, 1, 0),
    IVec3::new(0, 1, 1),
    IVec3::new(0, 1, -1),
];
const SIDES: [IVec3; 4] = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z];

/// A swap across a chunk border, applied after the phase if still valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub src: WorldPos,
    pub dst: WorldPos,
    pub src_material: MaterialId,
    pub dst_material: MaterialId,
}

/// Per-tick constants shared by every task.
#[derive(Debug, Clone, Copy)]
pub struct TickCtx<'a> {
    pub table: &'a MaterialTable,
    pub seed: u64,
    pub tick: u64,
}

/// What one chunk task produced.
#[derive(Debug, Default)]
pub struct TaskOut {
    /// Cells of the chunk that changed (local coordinates).
    pub changed: Aabb,
    pub outbox: Vec<Move>,
    pub moves: u32,
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// A chunk plus read-only access to its neighbours.
pub struct View<'a> {
    pub center: &'a mut Chunk,
    /// Indexed `(dx+1) + (dy+1)*3 + (dz+1)*9`; the centre slot is unused.
    pub neighbors: [Option<&'a Chunk>; 27],
    /// World position of the chunk's minimum corner.
    pub origin: I64Vec3,
}

impl std::fmt::Debug for View<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("origin", &self.origin)
            .finish()
    }
}

#[inline]
fn inside(p: IVec3) -> bool {
    (p.x as u32) < N as u32 && (p.y as u32) < N as u32 && (p.z as u32) < N as u32
}

#[inline]
fn local(p: IVec3) -> LocalPos {
    LocalPos::new(p.x as u8, p.y as u8, p.z as u8)
}

impl View<'_> {
    /// Material at chunk-local `p` (may be just outside the chunk); `None` if that
    /// neighbour is not loaded.
    #[inline]
    fn get(&self, p: IVec3) -> Option<MaterialId> {
        if inside(p) {
            return Some(self.center.get(local(p)));
        }
        let c = p.div_euclid(IVec3::splat(N));
        let slot = (c.x + 1) + (c.y + 1) * 3 + (c.z + 1) * 9;
        let chunk = self.neighbors.get(slot as usize).copied().flatten()?;
        Some(chunk.get(local(p.rem_euclid(IVec3::splat(N)))))
    }
}

/// Processes the cells of `region` (local coordinates) of the view's chunk.
pub fn process_chunk(
    view: &mut View<'_>,
    moved: &mut MovedBits,
    region: Aabb,
    ctx: &TickCtx<'_>,
) -> TaskOut {
    let _span = tracing::trace_span!("automaton_chunk").entered();
    let region = region.intersect(Aabb::CHUNK);
    let mut sweep = Sweep {
        view,
        moved,
        ctx,
        out: TaskOut::default(),
    };
    if region.is_empty() {
        return sweep.out;
    }
    // Alternate the horizontal sweep direction every tick to avoid drift bias.
    let flip_x = ctx.tick & 1 == 1;
    let flip_z = ctx.tick & 2 == 2;
    let (lo, hi) = (region.min, region.max);
    // Bottom-up: falling columns move as a whole in one tick.
    for y in lo.y..=hi.y {
        for iz in 0..=(hi.z - lo.z) {
            let z = if flip_z { hi.z - iz } else { lo.z + iz };
            let len = hi.x - lo.x + 1;
            let mut ix = 0;
            while ix < len {
                let x = if flip_x { hi.x - ix } else { lo.x + ix };
                let p = IVec3::new(x, y, z);
                // On entering a brick, skip it whole if nothing in it can move.
                let entering = ix == 0 || (x & 7) == if flip_x { 7 } else { 0 };
                if entering && sweep.inert_brick(p) {
                    ix += if flip_x { (x & 7) + 1 } else { 8 - (x & 7) };
                    continue;
                }
                sweep.cell(p);
                ix += 1;
            }
        }
    }
    sweep.out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Down,
    Up,
    Side,
}

/// Visits `dirs` in a pseudo-random order derived from `r` (rotation + reversal).
#[inline]
fn shuffled(dirs: &[IVec3; 4], r: u32) -> impl Iterator<Item = IVec3> + '_ {
    let start = (r & 3) as usize;
    let reverse = r & 4 != 0;
    (0..4).map(move |i| {
        let k = if reverse { 4 - i } else { i };
        dirs[(start + k) % 4]
    })
}

/// State of one chunk sweep.
struct Sweep<'s, 'v> {
    view: &'s mut View<'v>,
    moved: &'s mut MovedBits,
    ctx: &'s TickCtx<'s>,
    out: TaskOut,
}

impl Sweep<'_, '_> {
    #[inline]
    fn cell(&mut self, p: IVec3) {
        let index = local(p).index();
        if self.moved[index >> 6] & (1 << (index & 63)) != 0 {
            return;
        }
        let m = self.view.center.get(local(p));
        if m.is_air() {
            return;
        }
        let props = self.ctx.table.get(m);
        match props.kind {
            Kind::Static => {}
            Kind::Powder => {
                self.fall(p, m, props);
            }
            Kind::Liquid => self.liquid(p, m, props),
            Kind::Gas => self.gas(p, m, props),
        }
    }

    /// Whether the brick containing `p` is uniform air or a uniform static solid
    /// (no voxel in it can move by itself this tick).
    #[inline]
    fn inert_brick(&self, p: IVec3) -> bool {
        match self.view.center.brick(local(p).brick_index()) {
            BrickRef::Uniform(m) => m.is_air() || self.ctx.table.get(m).kind == Kind::Static,
            BrickRef::Dense(_) => false,
        }
    }

    fn random(&self, p: IVec3) -> u32 {
        rng::hash(
            self.ctx.seed,
            self.ctx.tick,
            self.view.origin + p.as_i64vec3(),
        )
    }

    /// The material at `q` if a voxel of `m` may swap into it moving along `dir`.
    #[inline]
    fn enterable(&self, m: MaterialId, props: Props, q: IVec3, dir: Dir) -> Option<MaterialId> {
        let t = self.view.get(q)?;
        if t == m {
            return None;
        }
        let tp = self.ctx.table.get(t);
        if !matches!(tp.kind, Kind::Liquid | Kind::Gas) {
            return None;
        }
        let ok = match dir {
            Dir::Down => props.density > tp.density,
            Dir::Up => props.density < tp.density,
            // Sideways only into gases; gases themselves only spread into air.
            Dir::Side => tp.kind == Kind::Gas && (props.kind != Kind::Gas || t.is_air()),
        };
        ok.then_some(t)
    }

    /// Swaps the voxel at `p` (inside the chunk, material `m`) with `q` (material `t`).
    #[inline]
    fn swap(&mut self, p: IVec3, q: IVec3, m: MaterialId, t: MaterialId) {
        self.out.moves += 1;
        if inside(q) {
            self.view.center.set(local(p), t);
            self.view.center.set(local(q), m);
            let i = local(q).index();
            self.moved[i >> 6] |= 1 << (i & 63);
            self.out.changed.include(p);
            self.out.changed.include(q);
        } else {
            let i = local(p).index();
            self.moved[i >> 6] |= 1 << (i & 63);
            self.out.outbox.push(Move {
                src: WorldPos(self.view.origin + p.as_i64vec3()),
                dst: WorldPos(self.view.origin + q.as_i64vec3()),
                src_material: m,
                dst_material: t,
            });
        }
    }

    /// First of `dirs` (random order) the voxel can move into, then swaps.
    fn try_dirs(
        &mut self,
        p: IVec3,
        m: MaterialId,
        props: Props,
        dirs: &[IVec3; 4],
        r: u32,
        dir: Dir,
    ) -> bool {
        for d in shuffled(dirs, r) {
            let q = p + d;
            if let Some(t) = self.enterable(m, props, q, dir) {
                self.swap(p, q, m, t);
                return true;
            }
        }
        false
    }

    /// Down, then the lower diagonals. Returns whether it moved.
    fn fall(&mut self, p: IVec3, m: MaterialId, props: Props) -> bool {
        let below = p - IVec3::Y;
        if let Some(t) = self.enterable(m, props, below, Dir::Down) {
            self.swap(p, below, m, t);
            return true;
        }
        // Randomness is only needed to pick a direction.
        let r = self.random(p);
        self.try_dirs(p, m, props, &DOWN_DIAGONALS, r, Dir::Down)
    }

    fn liquid(&mut self, p: IVec3, m: MaterialId, props: Props) {
        if self.fall(p, m, props) {
            return;
        }
        let r = self.random(p);
        let reach = i32::from(props.dispersion);
        for d in shuffled(&SIDES, r >> 3) {
            let mut target = None;
            for k in 1..=reach {
                let q = p + d * k;
                let Some(t) = self.enterable(m, props, q, Dir::Side) else {
                    break;
                };
                target = Some((q, t));
                // Stop at an edge: the voxel falls from there next tick.
                if self.enterable(m, props, q - IVec3::Y, Dir::Down).is_some() {
                    break;
                }
            }
            if let Some((q, t)) = target {
                self.swap(p, q, m, t);
                return;
            }
        }
    }

    fn gas(&mut self, p: IVec3, m: MaterialId, props: Props) {
        let above = p + IVec3::Y;
        if let Some(t) = self.enterable(m, props, above, Dir::Up) {
            self.swap(p, above, m, t);
            return;
        }
        let r = self.random(p);
        if self.try_dirs(p, m, props, &UP_DIAGONALS, r, Dir::Up) {
            return;
        }
        self.try_dirs(p, m, props, &SIDES, r >> 3, Dir::Side);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffled_is_a_permutation() {
        for r in 0..8 {
            let mut v: Vec<IVec3> = shuffled(&SIDES, r).collect();
            v.sort_by_key(|d| (d.x, d.z));
            let mut all = SIDES.to_vec();
            all.sort_by_key(|d| (d.x, d.z));
            assert_eq!(v, all, "r = {r}");
        }
    }
}
