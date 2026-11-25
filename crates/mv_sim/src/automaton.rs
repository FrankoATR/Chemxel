//! Cellular automaton rules for one chunk (PROMPT.md §6.2).
//!
//! A task owns its chunk mutably and sees its 26 neighbours read-only (they are never
//! processed in the same checkerboard phase). Moves that stay inside the chunk are
//! applied immediately; moves into a neighbour are queued as [`Move`]s and applied
//! serially after the phase (see `Simulation::step`).
//!
//! Rules by [`Kind`]:
//! * **Powder:** fall; else slide to one of the 4 lower diagonals (random order).
//! * **Liquid** (ADR 014): every liquid voxel has a fill level (1..=`LEVELS`). It
//!   falls whole (with its level) or slides down a diagonal; else pours into a partly
//!   filled voxel of the same liquid below; else shares its level with its 4
//!   neighbours: half the difference with the same liquid, half of itself into air
//!   (only over an edge, or if it holds at least `min_split` levels: surface
//!   tension). Viscous liquids (`dispersion` < 4) share only some ticks.
//! * **Gas** (and fire): rise; else upper diagonals; else drift one cell sideways.
//!
//! "Moving" is always a swap with the target: a denser material sinks through a
//! lighter fluid (sand through water, water through air) and the fluid takes its
//! place. Unloaded neighbours act as walls.
//!
//! Before moving, a voxel may react with a face neighbour (ADR 008: the reaction
//! table, O(1) lookup) or decay at the end of its lifetime. Voxels that could still
//! react or decay are reported as `live`, so their chunk stays awake even if nothing
//! moved.

use glam::{I64Vec3, IVec3};
use mv_core::consts::CHUNK_SIZE;
use mv_core::{LocalPos, WorldPos};
use mv_world::{BrickRef, Chunk, LEVELS, MaterialId};

use crate::aabb::Aabb;
use crate::rng;
use crate::table::{Kind, MaterialTable, Props, Rule, partial_change};
use crate::thermal::{ThermalChunk, ambient_change, cell_of, unstable_at};

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
const FACES: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

const SALT_REACT: u64 = 0x5EAC_7104;
const SALT_DECAY: u64 = 0xDECA_7000;
const SALT_EVAPORATE: u64 = 0xE7A9_0A7E;
const SALT_VISCOUS: u64 = 0x3A9D_E512;
/// Smallest level difference that makes a liquid share (3 % of a voxel, below
/// what the renderer shows): without it, an endless tail of invisible one- and
/// two-level flows keeps big lakes awake.
const SHARE_MIN: u8 = 8;
/// Thinnest film that pours over an edge (6 % of a voxel).
const POUR_MIN: u8 = 16;
/// Salt of the roll deciding what a partly filled liquid voxel changes into.
pub const SALT_PARTIAL: u64 = 0x9A27_1A15;

/// A change across a chunk border, applied after the phase if both cells still hold
/// what the task saw (materials and fill levels): a swap (movement), a liquid
/// sharing its level, or a reaction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Move {
    pub src: WorldPos,
    pub dst: WorldPos,
    pub src_material: MaterialId,
    pub dst_material: MaterialId,
    /// Fill levels seen by the task, and those to write.
    pub src_level: u8,
    pub dst_level: u8,
    pub src_level_into: u8,
    pub dst_level_into: u8,
    /// For reactions: the products and the extra heat. A swap leaves them as the
    /// swapped materials and `heat = 0`.
    pub src_into: MaterialId,
    pub dst_into: MaterialId,
    pub heat: f32,
    pub heat_cap: f32,
    pub react: bool,
}

/// Per-tick constants shared by every task.
#[derive(Debug, Clone, Copy)]
pub struct TickCtx<'a> {
    pub table: &'a MaterialTable,
    pub seed: u64,
    pub tick: u64,
    pub ambient: f32,
    pub hysteresis: f32,
}

/// What one chunk task produced.
#[derive(Debug, Default)]
pub struct TaskOut {
    /// Cells of the chunk that changed (local coordinates).
    pub changed: Aabb,
    /// Cells that did not change but must be processed again next tick (they may
    /// still react or decay).
    pub live: Aabb,
    pub outbox: Vec<Move>,
    pub moves: u32,
    pub reactions: u32,
    pub decays: u32,
    /// Loose liquid voxels that evaporated (or crusted over).
    pub evaporations: u32,
    /// Energy not conserved by in-place changes (ADR 007).
    pub imbalance: f64,
    /// Chunk without a field: reaction heat to apply once it has one, per cell
    /// (energy, temperature cap).
    pub heat: Vec<(usize, f32, f32)>,
    /// Chunk without a field: a product is unstable at ambient and needs one.
    pub needs_field: bool,
    /// Cells where a solid disappeared (structural integrity must check them).
    pub solid_lost: Vec<IVec3>,
    /// Gases with a ceiling whose lifetime ended: (world x, z, material) to place at
    /// the ceiling height above (vapour condensing into the cloud layer).
    pub deposits: Vec<(i64, i64, MaterialId)>,
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
    /// Temperature field of the chunk, if it has one (moves carry energy).
    pub thermal: Option<&'a mut ThermalChunk>,
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

    /// Fill level at chunk-local `p` (full if not loaded).
    #[inline]
    fn level(&self, p: IVec3) -> u8 {
        if inside(p) {
            return self.center.level(local(p));
        }
        let c = p.div_euclid(IVec3::splat(N));
        let slot = (c.x + 1) + (c.y + 1) * 3 + (c.z + 1) * 9;
        self.neighbors
            .get(slot as usize)
            .copied()
            .flatten()
            .map_or(LEVELS, |chunk| {
                chunk.level(local(p.rem_euclid(IVec3::splat(N))))
            })
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
    // Materials that may be around: reactions are only looked up for voxels with a
    // partner in this set.
    let present = view
        .neighbors
        .iter()
        .flatten()
        .fold(view.center.materials_mask(), |acc, c| {
            acc | c.materials_mask()
        });
    let mut sweep = Sweep {
        view,
        moved,
        ctx,
        present,
        out: TaskOut::default(),
    };
    if region.is_empty() {
        return sweep.out;
    }
    // Alternate the horizontal sweep direction every tick to avoid drift bias.
    let flip_x = ctx.tick & 1 == 1;
    let flip_z = ctx.tick & 2 == 2;
    let (lo, hi) = (region.min, region.max);
    // The 8 voxels of the current row inside the current brick, read once on
    // entering it so air is skipped without touching the chunk again. The copy may
    // go stale as voxels move, but only towards "moved this tick" voxels (every
    // voxel that arrives somewhere not yet visited is marked), which are skipped
    // anyway.
    let mut row = [MaterialId::AIR; 8];
    // Bottom-up: falling columns move as a whole in one tick.
    for y in lo.y..=hi.y {
        for iz in 0..=(hi.z - lo.z) {
            let z = if flip_z { hi.z - iz } else { lo.z + iz };
            let len = hi.x - lo.x + 1;
            let mut ix = 0;
            while ix < len {
                let x = if flip_x { hi.x - ix } else { lo.x + ix };
                let p = IVec3::new(x, y, z);
                let entering = ix == 0 || (x & 7) == if flip_x { 7 } else { 0 };
                if entering {
                    // Skip the brick's segment whole if nothing in it can move.
                    if sweep.inert_brick(p) {
                        ix += if flip_x { (x & 7) + 1 } else { 8 - (x & 7) };
                        continue;
                    }
                    row = match sweep.view.center.brick(local(p).brick_index()) {
                        BrickRef::Uniform(m) => [m; 8],
                        BrickRef::Dense(v) => {
                            let base = ((y & 7) * 8 + (z & 7) * 64) as usize;
                            v[base..base + 8].try_into().unwrap_or([MaterialId::AIR; 8])
                        }
                    };
                }
                if !row[(x & 7) as usize].is_air() {
                    sweep.cell(p);
                }
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
    /// Superset of the materials in the 3×3×3 chunks (`mv_world::material_bit`).
    present: u64,
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
        if props.partners & self.present != 0 && self.react(p, m) {
            return;
        }
        // Still voxels with a lifetime (clouds) fade in the slow pass instead, so
        // they do not keep their chunk awake.
        if props.decay != 0 && !(props.kind == Kind::Gas && !props.drifts) {
            if self.random_salted(p, SALT_DECAY) < props.decay {
                self.change_in_place(p, m, props.decays_into);
                self.out.decays += 1;
                if props.ceiling != i64::MAX {
                    // Vapour condenses into the cloud layer above (ADR 012).
                    let wp = self.view.origin + p.as_i64vec3();
                    self.out.deposits.push((wp.x, wp.z, props.ceiling_into));
                }
                return;
            }
            self.out.live.include(p);
        }
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

    fn random_salted(&self, p: IVec3, salt: u64) -> u32 {
        rng::hash(
            self.ctx.seed ^ salt,
            self.ctx.tick,
            self.view.origin + p.as_i64vec3(),
        )
    }

    /// Temperature of the cell containing `p` (inside the chunk).
    #[inline]
    fn temperature(&self, p: IVec3) -> f32 {
        self.view
            .thermal
            .as_deref()
            .map_or(self.ctx.ambient, |f| f.temp[cell_of(p)])
    }

    /// Tries the reactions of `m` at `p` with its 6 face neighbours (random start).
    /// Returns whether one happened; marks `p` live if a partner was present.
    fn react(&mut self, p: IVec3, m: MaterialId) -> bool {
        let table = self.ctx.table;
        let r = self.random_salted(p, SALT_REACT);
        let start = (r % 6) as usize;
        let mut partner = false;
        for k in 0..6 {
            let q = p + FACES[(start + k) % 6];
            let Some(t) = self.view.get(q) else {
                continue;
            };
            let Some(rule) = table.reaction(m, t).copied() else {
                continue;
            };
            partner = true;
            let temp = self.temperature(p);
            if temp < rule.min_temp || temp > rule.max_temp {
                continue;
            }
            if self.random_salted(p, SALT_REACT.wrapping_add(k as u64 + 1)) >= rule.chance {
                continue;
            }
            self.apply_reaction(p, q, m, t, &rule);
            return true;
        }
        if partner {
            self.out.live.include(p);
        }
        false
    }

    fn apply_reaction(&mut self, p: IVec3, q: IVec3, a: MaterialId, b: MaterialId, rule: &Rule) {
        self.out.reactions += 1;
        let i = local(p).index();
        self.moved[i >> 6] |= 1 << (i & 63);
        if !inside(q) {
            self.out.outbox.push(Move {
                src: WorldPos(self.view.origin + p.as_i64vec3()),
                dst: WorldPos(self.view.origin + q.as_i64vec3()),
                src_material: a,
                dst_material: b,
                src_level: self.view.level(p),
                dst_level: self.view.level(q),
                src_level_into: LEVELS,
                dst_level_into: LEVELS,
                src_into: rule.into_a,
                dst_into: rule.into_b,
                heat: rule.heat,
                heat_cap: rule.heat_cap,
                react: true,
            });
            return;
        }
        self.change_in_place(p, a, rule.into_a);
        self.change_in_place(q, b, rule.into_b);
        // The heat goes to `b`, the voxel acted upon (a flame heats what it touches).
        if rule.heat != 0.0 {
            match self.view.thermal.as_deref_mut() {
                Some(f) => {
                    let added = f.add_heat(cell_of(q), rule.heat, rule.heat_cap);
                    self.out.imbalance += f64::from(added);
                }
                None => self.out.heat.push((cell_of(q), rule.heat, rule.heat_cap)),
            }
        }
    }

    /// Turns the voxel at `p` (inside the chunk) from `old` into `new`, with the
    /// energy bookkeeping of an in-place change. A partly filled liquid keeps its
    /// amount on average (see [`partial_change`]).
    fn change_in_place(&mut self, p: IVec3, old: MaterialId, new: MaterialId) {
        let i = local(p).index();
        self.moved[i >> 6] |= 1 << (i & 63);
        if old == new {
            return;
        }
        let table = self.ctx.table;
        let level = self.view.center.level(local(p));
        let (new, new_level) =
            partial_change(table, new, level, self.random_salted(p, SALT_PARTIAL));
        if table.get(old).kind == Kind::Static && table.get(new).kind != Kind::Static {
            self.out.solid_lost.push(p);
        }
        let (po, pn) = (table.thermal(old), table.thermal(new));
        match self.view.thermal.as_deref_mut() {
            Some(f) => self.out.imbalance += f.change_voxel(cell_of(p), po, pn, f32::INFINITY),
            None => {
                self.out.imbalance += ambient_change(po, pn, self.ctx.ambient);
                self.out.needs_field |=
                    unstable_at(table, new, self.ctx.ambient, self.ctx.hysteresis);
            }
        }
        self.view.center.set_with_level(local(p), new, new_level);
        self.out.changed.include(p);
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
        let (lp, lq) = (self.view.level(p), self.view.level(q));
        if inside(q) {
            self.view.center.set_with_level(local(p), t, lq);
            self.view.center.set_with_level(local(q), m, lp);
            if let Some(field) = self.view.thermal.as_deref_mut() {
                let table = self.ctx.table;
                field.advect(cell_of(p), cell_of(q), table.thermal(m), table.thermal(t));
            }
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
                src_level: lp,
                dst_level: lq,
                src_level_into: lq,
                dst_level_into: lp,
                src_into: t,
                dst_into: m,
                heat: 0.0,
                heat_cap: f32::INFINITY,
                react: false,
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

    /// Liquids (ADR 014): fall whole, else pour into a partly filled voxel of the
    /// same liquid below (and keep going with the rest), else share their fill
    /// level with the 4 neighbours (partly filled ones with the lowest voxel up to
    /// `dispersion` cells along the surface). A voxel splits into air beside it
    /// over an edge (pours down, unless thinner than `POUR_MIN`), on top of more of
    /// the same liquid, or over the ground if it holds at least `min_split` levels,
    /// so a puddle stops spreading instead of creeping on forever. Thin films on
    /// the ground evaporate slowly. Once no neighbour differs by `SHARE_MIN` levels
    /// or more, the chunk sleeps.
    fn liquid(&mut self, p: IVec3, m: MaterialId, props: Props) {
        if self.fall(p, m, props) {
            return;
        }
        let mut f = self.view.center.level(local(p));
        let below = p - IVec3::Y;
        if self.view.get(below) == Some(m) {
            let fb = self.view.level(below);
            if fb < LEVELS {
                // Fill the voxel below, then keep sharing what is left sideways:
                // the edge of a mound must spill outwards, not only trickle down
                // into the layer under it.
                let amount = f.min(LEVELS - fb);
                if !self.transfer(p, below, m, m, f, fb, amount) || amount == f {
                    return;
                }
                f -= amount;
            }
        }
        // Viscous liquids share only `dispersion`/4 of the ticks.
        let sluggish = props.dispersion < 4
            && (self.random_salted(p, SALT_VISCOUS) >> 30) >= u32::from(props.dispersion);
        // Half a voxel (three quarters for viscous liquids) before a puddle spreads
        // over the ground: thinner sheets would multiply the voxels to simulate.
        let min_split = if props.dispersion >= 4 {
            LEVELS / 2
        } else {
            LEVELS / 4 * 3
        };
        let r = self.random(p);
        // Long reach only for partly filled voxels (a lake's top layer, which must
        // level out); full ones inside a pool only push into their neighbours.
        let reach = if f < LEVELS {
            i32::from(props.dispersion.max(1))
        } else {
            1
        };
        let mut shared = false;
        for d in shuffled(&SIDES, r >> 3) {
            let mut q = p + d;
            let Some(t) = self.view.get(q) else {
                continue;
            };
            // (amount to move, level at the target, material at the target)
            let (amount, fq, t) = if t == m {
                // Share with the lowest voxel up to `dispersion` cells along the
                // same surface: a lake levels out `dispersion`² times faster than
                // with the nearest neighbour only (and flatter, see ADR 014).
                let mut fq = self.view.level(q);
                for k in 2..=reach {
                    let next = p + d * k;
                    if self.view.get(next) != Some(m) {
                        break;
                    }
                    let level = self.view.level(next);
                    if level < fq {
                        (q, fq) = (next, level);
                    }
                }
                let diff = f.saturating_sub(fq);
                (if diff >= SHARE_MIN { diff / 2 } else { 0 }, fq, m)
            } else if t.is_air() {
                let under = q - IVec3::Y;
                let on_pool = self.view.get(under) == Some(m);
                if on_pool && self.view.level(under) < LEVELS {
                    // Onto a lower, partly filled voxel of the same liquid.
                    (if f >= 2 { f / 2 } else { f }, 0, t)
                } else if self.enterable(m, props, under, Dir::Down).is_some() {
                    // Over an edge: pour half (all of a small remainder). A film
                    // thinner than `POUR_MIN` clings to the ground instead (and
                    // evaporates), or rough slopes would drip forever.
                    if f >= 2 * POUR_MIN {
                        (f / 2, 0, t)
                    } else if f >= POUR_MIN {
                        (f, 0, t)
                    } else {
                        (0, 0, t)
                    }
                } else if on_pool && self.view.get(below) == Some(m) {
                    // A film on top of a full layer: pour straight into a lower,
                    // partly filled voxel of that layer up to `dispersion` cells
                    // away (else the film only drains through the full voxels
                    // under it, very slowly); or spread over the layer.
                    let mut lower = None;
                    for k in 2..=reach {
                        let c = p + d * k;
                        if !self.view.get(c).is_some_and(MaterialId::is_air)
                            || self.view.get(c - IVec3::Y) != Some(m)
                        {
                            break;
                        }
                        let level = self.view.level(c - IVec3::Y);
                        if level < LEVELS {
                            lower = Some((c - IVec3::Y, level));
                            break;
                        }
                    }
                    match lower {
                        Some((c, level)) => {
                            q = c;
                            (f.min(LEVELS - level), level, m)
                        }
                        None if f >= SHARE_MIN => (f / 2, 0, t),
                        None => (0, 0, t),
                    }
                } else if f >= min_split || (f >= SHARE_MIN && on_pool) {
                    // Over ground only past the surface tension threshold.
                    (f / 2, 0, t)
                } else {
                    (0, 0, t)
                }
            } else {
                (0, 0, t)
            };
            if amount == 0 {
                continue;
            }
            if sluggish {
                // It would flow: try again next tick.
                self.out.live.include(p);
                return;
            }
            // Across a border the change is applied after the phase, against the
            // level seen now: share no further this tick.
            if !self.transfer(p, q, m, t, f, fq, amount) {
                return;
            }
            shared = true;
            f -= amount;
            if f == 0 {
                return;
            }
        }
        if shared {
            return;
        }
        // At rest. A thin film on the ground (not a pool's top layer) evaporates.
        if props.evaporate != 0
            && f < min_split
            && self.view.get(below) != Some(m)
            && self.view.get(p + IVec3::Y).is_some_and(MaterialId::is_air)
        {
            if self.random_salted(p, SALT_EVAPORATE) < props.evaporate {
                self.change_in_place(p, m, props.evaporates_into);
                self.out.evaporations += 1;
            } else {
                self.out.live.include(p);
            }
        }
    }

    /// Moves `amount` levels of liquid `m` from `p` (level `fp`, inside the chunk)
    /// to `q` (material `t`: `m` with level `fq`, or air). `p` becomes air if
    /// emptied. Energy moves with the liquid where materials change. Returns
    /// `false` if `q` is across the border (queued, applied after the phase).
    #[allow(clippy::too_many_arguments)]
    fn transfer(
        &mut self,
        p: IVec3,
        q: IVec3,
        m: MaterialId,
        t: MaterialId,
        fp: u8,
        fq: u8,
        amount: u8,
    ) -> bool {
        self.out.moves += 1;
        let (lp, lq) = (fp - amount, fq + amount);
        let src_into = if lp == 0 { MaterialId::AIR } else { m };
        let lp = if lp == 0 { LEVELS } else { lp };
        let i = local(p).index();
        self.moved[i >> 6] |= 1 << (i & 63);
        if !inside(q) {
            self.out.outbox.push(Move {
                src: WorldPos(self.view.origin + p.as_i64vec3()),
                dst: WorldPos(self.view.origin + q.as_i64vec3()),
                src_material: m,
                dst_material: t,
                src_level: fp,
                dst_level: if t == m { fq } else { LEVELS },
                src_level_into: lp,
                dst_level_into: lq,
                src_into,
                dst_into: m,
                heat: 0.0,
                heat_cap: f32::INFINITY,
                react: false,
            });
            return false;
        }
        self.view.center.set_with_level(local(p), src_into, lp);
        self.view.center.set_with_level(local(q), m, lq);
        // Energy. A voxel's heat capacity (and latent heat) does not depend on its
        // level, so a liquid voxel appearing in air (or a source emptying into more
        // of the same liquid) creates (or destroys) energy: accounted (ADR 007).
        let table = self.ctx.table;
        let (pm, pt, pa) = (
            table.thermal(m),
            table.thermal(t),
            table.thermal(MaterialId::AIR),
        );
        let (cp, cq) = (cell_of(p), cell_of(q));
        let ambient = self.ctx.ambient;
        let field = self.view.thermal.as_deref_mut();
        let created = match (src_into == m, t == m) {
            // The whole voxel moved into air: like a swap.
            (false, false) => {
                if let Some(f) = field {
                    f.advect(cp, cq, pm, pt);
                }
                None
            }
            // New liquid where there was air, at the source's temperature.
            (true, false) => Some((cq, pt, pm, field)),
            // The source emptied into more of the same liquid.
            (false, true) => Some((cp, pm, pa, field)),
            (true, true) => None,
        };
        if let Some((cell, old, new, field)) = created {
            self.out.imbalance += match field {
                Some(f) => {
                    let t_in = f.temp[cp];
                    let before = f.energy_of(cell);
                    f.replace_voxel(cell, old, new, t_in);
                    f.energy_of(cell) - before + f64::from(new.latent - old.latent)
                }
                None => ambient_change(old, new, ambient),
            };
        }
        let j = local(q).index();
        self.moved[j >> 6] |= 1 << (j & 63);
        self.out.changed.include(p);
        self.out.changed.include(q);
        true
    }

    fn gas(&mut self, p: IVec3, m: MaterialId, props: Props) {
        // Vapour reaching cloud height becomes cloud, and so does vapour that rises
        // into the underside of a cloud (clouds thicken downwards).
        if self.view.origin.y + i64::from(p.y) >= props.ceiling
            || (props.ceiling != i64::MAX
                && self.view.get(p + IVec3::Y) == Some(props.ceiling_into))
        {
            self.change_in_place(p, m, props.ceiling_into);
            return;
        }
        if !props.drifts {
            return;
        }
        // Rise up to `dispersion` cells per tick (1 for most gases; vapour climbs
        // faster so it spends less time crossing the air).
        let mut target = None;
        for k in 1..=i32::from(props.dispersion) {
            let q = p + IVec3::Y * k;
            match self.enterable(m, props, q, Dir::Up) {
                Some(t) => target = Some((q, t)),
                None => break,
            }
        }
        if let Some((q, t)) = target {
            self.swap(p, q, m, t);
            return;
        }
        let r = self.random(p);
        if self.try_dirs(p, m, props, &UP_DIAGONALS, r, Dir::Up) || !props.spreads {
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
