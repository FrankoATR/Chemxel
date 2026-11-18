//! Structural integrity (PROMPT.md §6.6, ADR 009).
//!
//! When solids disappear (edits, reactions, burning, melting), the region is queued.
//! Each tick a bounded flood fill (6-connected, over static solids) starts from the
//! solids of the queued regions. A component is **anchored** when the fill reaches
//! an anchor voxel (see [`Anchor`]), an unloaded chunk, or exceeds the size limits
//! (a huge component is part of the terrain). Anything else is floating: it is cut
//! out of the grid as a [`VoxelBody`] for the physics engine.
//!
//! The fill dives downwards first, so a piece standing on the ground finds its
//! anchor in a few dozen steps. Visited sets are per-chunk bitsets and chunk lookups
//! are cached, so the hot loop does no hashing per voxel in the common case.

use std::collections::{HashMap, VecDeque};

use glam::{I64Vec3, IVec3};
use mv_core::{ChunkPos, LocalPos, WorldPos};
use mv_world::{Chunk, MaterialId, World};

use crate::table::{Kind, MaterialTable};

/// Decides whether a solid voxel is anchored to the ground (e.g. "deeper than N
/// voxels below the generated terrain surface"). Must be cheap.
pub type Anchor = Box<dyn Fn(WorldPos) -> bool + Send + Sync>;

/// A piece of solid cut out of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelBody {
    /// World position of the grid's minimum corner.
    pub origin: WorldPos,
    /// Grid size in voxels.
    pub size: IVec3,
    /// Materials, x fastest; air where the piece has no voxel.
    pub voxels: Vec<MaterialId>,
    /// Non-air voxels.
    pub count: usize,
}

impl VoxelBody {
    #[inline]
    pub fn index(&self, p: IVec3) -> usize {
        (p.x + p.y * self.size.x + p.z * self.size.x * self.size.y) as usize
    }

    /// Material at grid position `p` (air outside the grid).
    #[inline]
    pub fn get(&self, p: IVec3) -> MaterialId {
        if p.cmplt(IVec3::ZERO).any() || p.cmpge(self.size).any() {
            return MaterialId::AIR;
        }
        self.voxels[self.index(p)]
    }

    /// Grid positions of the non-air voxels.
    pub fn filled(&self) -> impl Iterator<Item = (IVec3, MaterialId)> + '_ {
        let s = self.size;
        self.voxels.iter().enumerate().filter_map(move |(i, &m)| {
            if m.is_air() {
                return None;
            }
            let i = i as i32;
            Some((IVec3::new(i % s.x, (i / s.x) % s.y, i / (s.x * s.y)), m))
        })
    }
}

/// Limits of the flood fill.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_voxels: usize,
    pub max_extent: i64,
    pub budget: usize,
}

/// One bit per voxel, per chunk.
#[derive(Debug, Default)]
struct Bits {
    chunks: Vec<(ChunkPos, Box<[u64; 4096]>)>,
    index: HashMap<ChunkPos, usize>,
    last: Option<(ChunkPos, usize)>,
}

impl Bits {
    fn slot(&mut self, c: ChunkPos) -> usize {
        if let Some((p, i)) = self.last
            && p == c
        {
            return i;
        }
        let i = match self.index.get(&c) {
            Some(&i) => i,
            None => {
                self.chunks.push((c, Box::new([0; 4096])));
                self.index.insert(c, self.chunks.len() - 1);
                self.chunks.len() - 1
            }
        };
        self.last = Some((c, i));
        i
    }

    fn get(&mut self, p: I64Vec3) -> bool {
        let (c, l) = WorldPos(p).to_chunk_local();
        let i = self.slot(c);
        let bit = l.index();
        self.chunks[i].1[bit >> 6] & (1 << (bit & 63)) != 0
    }

    fn set(&mut self, p: I64Vec3, on: bool) {
        let (c, l) = WorldPos(p).to_chunk_local();
        let i = self.slot(c);
        let bit = l.index();
        let word = &mut self.chunks[i].1[bit >> 6];
        if on {
            *word |= 1 << (bit & 63);
        } else {
            *word &= !(1 << (bit & 63));
        }
    }

    fn clear(&mut self) {
        // Keep the allocations: zero and reuse.
        for (_, b) in &mut self.chunks {
            b.fill(0);
        }
    }
}

/// Reads voxels with the last chunk cached.
struct Reader<'w> {
    world: &'w World,
    last: Option<(ChunkPos, Option<&'w Chunk>)>,
}

impl<'w> Reader<'w> {
    fn new(world: &'w World) -> Self {
        Self { world, last: None }
    }

    /// `None` if the chunk is not loaded.
    #[inline]
    fn get(&mut self, p: I64Vec3) -> Option<MaterialId> {
        let (c, l): (ChunkPos, LocalPos) = WorldPos(p).to_chunk_local();
        let chunk = match self.last {
            Some((lc, chunk)) if lc == c => chunk,
            _ => {
                let chunk = self.world.chunk(c);
                self.last = Some((c, chunk));
                chunk
            }
        };
        chunk.map(|ch| ch.get(l))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    Anchored,
    Floating,
}

/// Pending checks and scratch memory of the integrity pass.
pub(crate) struct Integrity {
    queue: VecDeque<(I64Vec3, I64Vec3)>,
    limits: Limits,
    anchor: Option<Anchor>,
    /// Voxels of anchored components found while processing the current region.
    done: Bits,
    /// Voxels of the component being filled.
    cur: Bits,
    stack: Vec<I64Vec3>,
    component: Vec<I64Vec3>,
    seeds: Vec<I64Vec3>,
}

impl std::fmt::Debug for Integrity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Integrity")
            .field("queue", &self.queue.len())
            .field("limits", &self.limits)
            .field("anchor", &self.anchor.is_some())
            .finish_non_exhaustive()
    }
}

/// Result of one integrity pass.
#[derive(Debug, Default)]
pub(crate) struct Pass {
    pub bodies: Vec<VoxelBody>,
    /// Voxels visited by the flood fills.
    pub visited: usize,
}

#[inline]
fn is_solid(table: &MaterialTable, m: MaterialId) -> bool {
    !m.is_air() && table.get(m).kind == Kind::Static
}

/// Neighbours in push order: down is pushed last so it is explored first.
const DIRS: [I64Vec3; 6] = [
    I64Vec3::Y,
    I64Vec3::X,
    I64Vec3::NEG_X,
    I64Vec3::Z,
    I64Vec3::NEG_Z,
    I64Vec3::NEG_Y,
];

impl Integrity {
    pub fn new(limits: Limits) -> Self {
        Self {
            queue: VecDeque::new(),
            limits,
            anchor: None,
            done: Bits::default(),
            cur: Bits::default(),
            stack: Vec::new(),
            component: Vec::new(),
            seeds: Vec::new(),
        }
    }

    pub fn set_anchor(&mut self, anchor: Option<Anchor>) {
        self.anchor = anchor;
    }

    /// Queues the world box `[lo, hi]` (where solids disappeared) for a check.
    pub fn push(&mut self, lo: I64Vec3, hi: I64Vec3) {
        let (lo, hi) = (lo.min(hi) - 1, lo.max(hi) + 1);
        // Merge with the last region if it overlaps (brush strokes repeat).
        if let Some((l, h)) = self.queue.back_mut()
            && l.cmple(hi).all()
            && lo.cmple(*h).all()
            && (h.max(hi) - l.min(lo)).max_element() <= 128
        {
            *l = l.min(lo);
            *h = h.max(hi);
            return;
        }
        self.queue.push_back((lo, hi));
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Processes queued regions within the voxel budget; floating components are
    /// removed from `world` and returned. A region, once started, is finished (so
    /// every region makes progress); the budget only defers the next ones.
    pub fn run(&mut self, world: &mut World, table: &MaterialTable) -> Pass {
        let _span = tracing::trace_span!("integrity").entered();
        let mut pass = Pass::default();
        let mut budget = self.limits.budget;
        while let Some((lo, hi)) = self.queue.pop_front() {
            if budget < self.limits.max_voxels {
                self.queue.push_front((lo, hi));
                break;
            }
            self.done.clear();
            // Seeds: the solids of the region.
            self.seeds.clear();
            let mut reader = Reader::new(world);
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        let p = I64Vec3::new(x, y, z);
                        if reader.get(p).is_some_and(|m| is_solid(table, m)) {
                            self.seeds.push(p);
                        }
                    }
                }
            }
            for s in 0..self.seeds.len() {
                let p = self.seeds[s];
                // Already part of an anchored component, or cut out with a body.
                if self.done.get(p) || !world.voxel(WorldPos(p)).is_some_and(|m| is_solid(table, m))
                {
                    continue;
                }
                let result = self.fill(world, table, p);
                let n = self.component.len();
                budget = budget.saturating_sub(n);
                pass.visited += n;
                for i in 0..n {
                    let q = self.component[i];
                    self.cur.set(q, false);
                    if result == Fill::Anchored {
                        self.done.set(q, true);
                    }
                }
                if result == Fill::Floating {
                    pass.bodies.push(extract(world, &self.component));
                }
            }
        }
        pass
    }

    /// Flood fill from solid `seed` into `self.component`.
    fn fill(&mut self, world: &World, table: &MaterialTable, seed: I64Vec3) -> Fill {
        self.stack.clear();
        self.component.clear();
        self.stack.push(seed);
        self.cur.set(seed, true);
        self.component.push(seed);
        let (mut bmin, mut bmax) = (seed, seed);
        let mut reader = Reader::new(world);
        while let Some(p) = self.stack.pop() {
            if self.anchor.as_ref().is_some_and(|a| a(WorldPos(p))) {
                return Fill::Anchored;
            }
            for d in DIRS {
                let q = p + d;
                let Some(m) = reader.get(q) else {
                    return Fill::Anchored; // touches unloaded world
                };
                if !is_solid(table, m) || self.cur.get(q) {
                    continue;
                }
                if self.done.get(q) {
                    return Fill::Anchored;
                }
                self.cur.set(q, true);
                self.component.push(q);
                bmin = bmin.min(q);
                bmax = bmax.max(q);
                if self.component.len() > self.limits.max_voxels
                    || (bmax - bmin).max_element() >= self.limits.max_extent
                {
                    return Fill::Anchored;
                }
                self.stack.push(q);
            }
        }
        Fill::Floating
    }
}

/// Cuts the voxels at `positions` out of `world` into a body.
fn extract(world: &mut World, positions: &[I64Vec3]) -> VoxelBody {
    let (mut lo, mut hi) = (positions[0], positions[0]);
    for &p in positions {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let size = (hi - lo + 1).as_ivec3();
    let mut body = VoxelBody {
        origin: WorldPos(lo),
        size,
        voxels: vec![MaterialId::AIR; (size.x * size.y * size.z) as usize],
        count: positions.len(),
    };
    for &p in positions {
        let m = world.voxel(WorldPos(p)).unwrap_or(MaterialId::AIR);
        let i = body.index((p - lo).as_ivec3());
        body.voxels[i] = m;
        world.set_voxel(WorldPos(p), MaterialId::AIR);
    }
    body
}
