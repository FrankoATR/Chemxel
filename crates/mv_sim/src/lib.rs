//! Headless voxel simulation.
//!
//! This crate must never depend on rendering or windowing: it has to run in tests
//! and, in the future, on a dedicated server. See `PROMPT.md` §6 and ADR 006.
//!
//! # Tick
//!
//! Only *active* chunks are simulated, and inside each only its dirty region (an
//! AABB of cells that changed last tick, grown by one cell). A chunk falls asleep
//! after `sleep_after_ticks` ticks without changes and is woken by edits
//! ([`Simulation::wake_box`]) or by changes next to it.
//!
//! Active chunks are processed in 8 phases, one per 2×2×2 parity class of their
//! chunk coordinates. Chunks of one class are never neighbours, so they run in
//! parallel (rayon): each task takes its chunk out of the [`World`] and reads its
//! neighbours, which nobody mutates during that phase. Moves into a neighbour are
//! queued and applied serially after the phase, in a deterministic order, if still
//! valid. Randomness is a hash of (seed, tick, position), so a tick's result does
//! not depend on thread scheduling.

mod aabb;
mod automaton;
mod rng;
mod table;

use std::collections::HashMap;
use std::sync::Arc;

use glam::{I64Vec3, IVec3};
use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::MaterialRegistry;
use mv_world::World;
use rayon::prelude::*;

pub use aabb::Aabb;
pub use table::{Kind, MaterialTable, Props};

use automaton::{MOVED_WORDS, Move, MovedBits, TaskOut, TickCtx, View};

/// Counters of the last tick, for the debug overlay and benchmarks.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickStats {
    pub active_chunks: usize,
    /// Cells inside the processed regions.
    pub cells_scanned: u64,
    pub moves: u32,
    /// Moves that crossed a chunk border (applied after their phase).
    pub border_moves: u32,
}

/// Simulation state of an awake chunk.
#[derive(Debug)]
struct ActiveChunk {
    /// Region processed this tick.
    region: Aabb,
    /// Region to process next tick (accumulated during this one).
    pending: Aabb,
    /// Last non-empty region, re-processed while idle before sleeping.
    last: Aabb,
    idle_ticks: u32,
    moved: Box<MovedBits>,
}

/// Owns the simulation state and advances it one fixed tick at a time.
#[derive(Debug)]
pub struct Simulation {
    tick: u64,
    seed: u64,
    sleep_after: u32,
    paused: bool,
    table: Arc<MaterialTable>,
    active: HashMap<ChunkPos, ActiveChunk>,
    /// Recycled "moved" bitsets (32 KiB each).
    spare_bits: Vec<Box<MovedBits>>,
    stats: TickStats,
}

impl Simulation {
    pub fn new(registry: &MaterialRegistry, config: &SimConfig) -> Self {
        Self::with_table(MaterialTable::from_registry(registry), config)
    }

    pub fn with_table(table: MaterialTable, config: &SimConfig) -> Self {
        Self {
            tick: 0,
            seed: config.seed,
            sleep_after: config.sleep_after_ticks,
            paused: false,
            table: Arc::new(table),
            active: HashMap::new(),
            spare_bits: Vec::new(),
            stats: TickStats::default(),
        }
    }

    /// Number of ticks simulated so far.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn stats(&self) -> TickStats {
        self.stats
    }

    pub fn active_chunks(&self) -> usize {
        self.active.len()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// While paused, [`Simulation::step`] does nothing (wake-ups are still recorded).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Wakes every loaded chunk overlapping the inclusive world box `[min, max]`
    /// (grown by one cell) and schedules those cells for the next tick.
    pub fn wake_box(&mut self, world: &World, min: WorldPos, max: WorldPos) {
        self.mark_dirty(world, min.0.min(max.0) - 1, min.0.max(max.0) + 1);
    }

    /// Adds the world box `[lo, hi]` to the pending region of every loaded chunk it
    /// overlaps, waking them.
    fn mark_dirty(&mut self, world: &World, lo: I64Vec3, hi: I64Vec3) {
        let (clo, chi) = (WorldPos(lo).chunk().0, WorldPos(hi).chunk().0);
        for cz in clo.z..=chi.z {
            for cy in clo.y..=chi.y {
                for cx in clo.x..=chi.x {
                    let pos = ChunkPos::new(cx, cy, cz);
                    if !world.contains(pos) {
                        continue;
                    }
                    let origin = pos.origin().0;
                    let local = Aabb::new((lo - origin).as_ivec3(), (hi - origin).as_ivec3())
                        .intersect(Aabb::CHUNK);
                    let entry = self.activate(pos);
                    entry.pending = entry.pending.union(local);
                }
            }
        }
    }

    fn activate(&mut self, pos: ChunkPos) -> &mut ActiveChunk {
        let spare = &mut self.spare_bits;
        self.active.entry(pos).or_insert_with(|| ActiveChunk {
            region: Aabb::EMPTY,
            pending: Aabb::EMPTY,
            last: Aabb::EMPTY,
            idle_ticks: 0,
            moved: spare.pop().unwrap_or_else(|| Box::new([0; MOVED_WORDS])),
        })
    }

    /// Advances the simulation by exactly one fixed tick.
    pub fn step(&mut self, world: &mut World) {
        if self.paused {
            return;
        }
        let _span = tracing::trace_span!("sim_step", tick = self.tick).entered();
        self.tick += 1;
        let mut stats = TickStats::default();

        let schedule = self.begin_tick(world);
        stats.active_chunks = self.active.len();
        let table = Arc::clone(&self.table);
        let ctx = TickCtx {
            table: &table,
            seed: self.seed,
            tick: self.tick,
        };

        for phase in schedule {
            if phase.is_empty() {
                continue;
            }
            let _span = tracing::trace_span!("sim_phase", chunks = phase.len()).entered();
            // Take the phase's chunks out so the rest of the world can be shared.
            let mut tasks: Vec<_> = phase
                .into_iter()
                .filter_map(|pos| {
                    let chunk = world.detach(pos)?;
                    let state = self.active.remove(&pos)?;
                    Some((pos, chunk, state))
                })
                .collect();
            let shared: &World = world;
            let outs: Vec<TaskOut> = tasks
                .par_iter_mut()
                .map(|(pos, chunk, state)| {
                    let mut view = View {
                        center: chunk,
                        neighbors: neighbors(shared, *pos),
                        origin: pos.origin().0,
                    };
                    automaton::process_chunk(&mut view, &mut state.moved, state.region, &ctx)
                })
                .collect();

            let mut finished = Vec::with_capacity(tasks.len());
            for (pos, chunk, state) in tasks {
                stats.cells_scanned += state.region.volume();
                world.attach(pos, chunk);
                self.active.insert(pos, state);
                finished.push(pos);
            }
            for (pos, out) in finished.into_iter().zip(outs) {
                stats.moves += out.moves;
                if !out.changed.is_empty() {
                    world.mark_edited(pos);
                    let origin = pos.origin().0;
                    let changed = out.changed.expand(1);
                    self.mark_dirty(
                        world,
                        origin + changed.min.as_i64vec3(),
                        origin + changed.max.as_i64vec3(),
                    );
                }
                for mv in &out.outbox {
                    if self.apply_move(world, mv) {
                        stats.border_moves += 1;
                    }
                }
            }
        }
        self.stats = stats;
    }

    /// Promotes pending regions, puts idle chunks to sleep and groups the active
    /// chunks into the 8 parity phases (sorted, so the order is deterministic).
    fn begin_tick(&mut self, world: &mut World) -> [Vec<ChunkPos>; 8] {
        let mut sleeping = Vec::new();
        for (&pos, state) in &mut self.active {
            if !world.contains(pos) {
                sleeping.push(pos);
                continue;
            }
            if state.pending.is_empty() {
                state.idle_ticks += 1;
                if state.idle_ticks > self.sleep_after {
                    sleeping.push(pos);
                    continue;
                }
                state.region = state.last;
            } else {
                state.idle_ticks = 0;
                state.region = state.pending;
                state.last = state.pending;
                state.pending = Aabb::EMPTY;
            }
            state.moved.fill(0);
        }
        for pos in sleeping {
            if let Some(state) = self.active.remove(&pos) {
                self.spare_bits.push(state.moved);
            }
            if let Some(chunk) = world.chunk_mut(pos) {
                chunk.compact();
            }
        }

        let mut phases: [Vec<ChunkPos>; 8] = Default::default();
        let mut positions: Vec<ChunkPos> = self.active.keys().copied().collect();
        positions.sort_by_key(|p| (p.0.z, p.0.y, p.0.x));
        for pos in positions {
            let c = pos.0.rem_euclid(IVec3::splat(2));
            phases[(c.x + c.y * 2 + c.z * 4) as usize].push(pos);
        }
        phases
    }

    /// Applies a queued border move if both cells still hold what the task saw.
    fn apply_move(&mut self, world: &mut World, mv: &Move) -> bool {
        if world.voxel(mv.src) != Some(mv.src_material)
            || world.voxel(mv.dst) != Some(mv.dst_material)
        {
            return false;
        }
        world.set_voxel(mv.src, mv.dst_material);
        world.set_voxel(mv.dst, mv.src_material);
        // The voxel must not move again this tick when its new chunk is processed.
        let (cpos, local) = mv.dst.to_chunk_local();
        let index = local.index();
        self.activate(cpos).moved[index >> 6] |= 1 << (index & 63);
        for p in [mv.src.0, mv.dst.0] {
            self.mark_dirty(world, p - 1, p + 1);
        }
        true
    }
}

/// The 26 neighbours of `pos` (centre slot empty).
fn neighbors(world: &World, pos: ChunkPos) -> [Option<&mv_world::Chunk>; 27] {
    let mut out = [None; 27];
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let slot = (dx + 1) + (dy + 1) * 3 + (dz + 1) * 9;
                out[slot as usize] = world.chunk(ChunkPos(pos.0 + IVec3::new(dx, dy, dz)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
