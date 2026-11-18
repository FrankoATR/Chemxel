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
//!
//! During that sweep voxels also react with their neighbours and decay at the end
//! of their lifetime (ADR 008). After it, the thermal pass diffuses heat, loses heat
//! to the environment, and applies phase changes and combustion (ADR 007, 008).

mod aabb;
mod automaton;
mod integrity;
mod rng;
mod table;
mod thermal;

use std::collections::HashMap;
use std::sync::Arc;

use glam::{I64Vec3, IVec3};
use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_world::World;
use rayon::prelude::*;

pub use aabb::Aabb;
pub use integrity::{Anchor, VoxelBody};
pub use table::{Kind, MaterialTable, Props, Rule, ThermalProps};
pub use thermal::{CELL, CELLS, CELLS_AXIS, CellArray};

use automaton::{MOVED_WORDS, Move, MovedBits, TaskOut, TickCtx, View};
use thermal::{
    DiffusionParams, ThermalChunk, TransitionParams, ambient_change, cell_of, unstable_at,
};

/// Counters of the last tick, for the debug overlay and benchmarks.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickStats {
    pub active_chunks: usize,
    /// Cells inside the processed regions.
    pub cells_scanned: u64,
    pub moves: u32,
    /// Moves that crossed a chunk border (applied after their phase).
    pub border_moves: u32,
    /// Chunks with a temperature field.
    pub thermal_chunks: usize,
    /// Voxels that changed material because of temperature (phase changes and
    /// combustion).
    pub transitions: u32,
    pub reactions: u32,
    /// Voxels whose lifetime expired.
    pub decays: u32,
    /// Flames emitted by burning voxels.
    pub flames: u32,
    /// Voxels visited by the structural integrity flood fill.
    pub integrity_visited: u32,
    /// Floating pieces cut out of the grid this tick.
    pub detached: u32,
    /// Wall time of each pass of the last tick, in milliseconds.
    pub automaton_ms: f32,
    pub thermal_ms: f32,
    pub integrity_ms: f32,
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

    thermal: HashMap<ChunkPos, ThermalChunk>,
    /// Recycled diffusion output buffers.
    thermal_scratch: Vec<Box<CellArray>>,
    ambient: f32,
    epsilon: f32,
    hysteresis: f32,
    transition_rate: f32,
    diffusion_rate: f32,
    /// Energy deliberately not conserved (phase changes, fields put to sleep).
    energy_imbalance: f64,

    integrity: integrity::Integrity,
    /// Floating pieces cut out of the grid, waiting for the physics engine.
    detached: Vec<VoxelBody>,
}

impl Simulation {
    pub fn new(registry: &MaterialRegistry, reactions: &ReactionSet, config: &SimConfig) -> Self {
        Self::with_table(MaterialTable::new(registry, reactions), config)
    }

    /// Replaces the material and reaction tables (hot reload). Ids must keep their
    /// meaning (see `MaterialRegistry::keeps_ids_of`); temperature fields keep their
    /// temperatures and get the new composition.
    pub fn set_materials(
        &mut self,
        registry: &MaterialRegistry,
        reactions: &ReactionSet,
        world: &World,
    ) {
        self.table = Arc::new(MaterialTable::new(registry, reactions));
        for (pos, field) in &mut self.thermal {
            if let Some(chunk) = world.chunk(*pos) {
                field.recompute(chunk, &self.table, Aabb::CHUNK);
            }
        }
        // Everything awake is re-examined with the new rules.
        for state in self.active.values_mut() {
            state.pending = Aabb::CHUNK;
        }
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
            thermal: HashMap::new(),
            thermal_scratch: Vec::new(),
            ambient: config.ambient_temperature,
            epsilon: config.thermal_epsilon,
            hysteresis: config.transition_hysteresis,
            transition_rate: config.transition_rate,
            diffusion_rate: config.diffusion_rate,
            energy_imbalance: 0.0,
            integrity: integrity::Integrity::new(
                integrity::Limits {
                    min_voxels: config.integrity_min_voxels as usize,
                    max_voxels: config.integrity_max_voxels as usize,
                    max_extent: i64::from(config.integrity_max_extent),
                    budget: config.integrity_budget_voxels as usize,
                },
                config.integrity_natural_period,
            ),
            detached: Vec::new(),
        }
    }

    /// Sets what counts as ground for structural integrity (besides unloaded chunks
    /// and components too big to move). Without one, only those two anchor.
    pub fn set_anchor(&mut self, anchor: Option<Anchor>) {
        self.integrity.set_anchor(anchor);
    }

    /// Floating pieces cut out of the grid since the last call (for the physics
    /// engine). Their voxels are already air in the world.
    pub fn take_detached(&mut self) -> Vec<VoxelBody> {
        std::mem::take(&mut self.detached)
    }

    /// Regions waiting for a structural integrity check.
    pub fn integrity_pending(&self) -> usize {
        self.integrity.pending()
    }

    /// Voxels were written into `[min, max]` by someone else than the editor (a
    /// rigid body put back into the grid): wakes the simulation, updates
    /// temperature fields and re-checks integrity there (the piece may be hanging).
    pub fn voxels_placed(&mut self, world: &World, min: WorldPos, max: WorldPos) {
        self.edited(world, min, max, None);
    }

    /// Like [`Simulation::voxels_placed`] but without an integrity check (a piece put
    /// back where it was cut out, e.g. when there is no room for more bodies).
    pub fn voxels_restored(&mut self, world: &World, min: WorldPos, max: WorldPos) {
        self.edited(world, min, max, Some(MaterialId::AIR));
    }

    pub fn ambient_temperature(&self) -> f32 {
        self.ambient
    }

    pub fn thermal_chunks(&self) -> usize {
        self.thermal.len()
    }

    /// Temperature fields (16³ cells of 4³ voxels) of the chunks that have one; every
    /// other chunk is at the ambient temperature.
    pub fn thermal_fields(&self) -> impl Iterator<Item = (ChunkPos, &CellArray)> {
        self.thermal.iter().map(|(p, f)| (*p, &*f.temp))
    }

    /// Temperature of the cell containing `pos`.
    pub fn temperature_at(&self, pos: WorldPos) -> f32 {
        let (c, l) = pos.to_chunk_local();
        self.thermal
            .get(&c)
            .map_or(self.ambient, |f| f.temp[cell_of(l.as_ivec3())])
    }

    /// Energy changed on purpose rather than conserved: volume-preserving phase
    /// changes and fields returned to ambient (ADR 007).
    pub fn energy_imbalance(&self) -> f64 {
        self.energy_imbalance
    }

    /// Total energy of the loaded world: sensible (capacity × temperature, ambient
    /// for chunks without a field) plus latent. Walks every voxel; for tests.
    pub fn total_energy(&self, world: &World) -> f64 {
        let mut total = 0.0;
        for (pos, chunk) in world.iter() {
            total += match self.thermal.get(&pos) {
                Some(f) => f.energy(),
                None => ThermalChunk::new(chunk, &self.table, self.ambient).energy(),
            };
            for b in 0..mv_core::consts::BRICKS_PER_CHUNK {
                total += match chunk.brick(b) {
                    mv_world::BrickRef::Uniform(m) => {
                        f64::from(self.table.thermal(m).latent) * 512.0
                    }
                    mv_world::BrickRef::Dense(v) => v
                        .iter()
                        .map(|&m| f64::from(self.table.thermal(m).latent))
                        .sum(),
                };
            }
        }
        total
    }

    /// Reacts to an external edit of the world box `[min, max]`: wakes the
    /// automaton there, updates the composition of temperature fields, and gives
    /// voxels of `placed` (if any) their base temperature. Removals (`placed ==
    /// None`) also queue a structural integrity check.
    pub fn edited(
        &mut self,
        world: &World,
        min: WorldPos,
        max: WorldPos,
        placed: Option<MaterialId>,
    ) {
        self.wake_box(world, min, max);
        let (lo, hi) = (min.0.min(max.0), min.0.max(max.0));
        if placed.is_none() {
            self.integrity.push(lo, hi);
        }
        let needs_field = placed.is_some_and(|m| {
            self.table.thermal(m).base_temperature.is_some()
                || unstable_at(&self.table, m, self.ambient, self.hysteresis)
        });
        let (clo, chi) = (WorldPos(lo).chunk().0, WorldPos(hi).chunk().0);
        for cz in clo.z..=chi.z {
            for cy in clo.y..=chi.y {
                for cx in clo.x..=chi.x {
                    let pos = ChunkPos::new(cx, cy, cz);
                    let Some(chunk) = world.chunk(pos) else {
                        continue;
                    };
                    let origin = pos.origin().0;
                    let region = Aabb::new((lo - origin).as_ivec3(), (hi - origin).as_ivec3())
                        .intersect(Aabb::CHUNK);
                    match self.thermal.get_mut(&pos) {
                        Some(f) => f.recompute(chunk, &self.table, region),
                        None if needs_field => {
                            let f = ThermalChunk::new(chunk, &self.table, self.ambient);
                            self.thermal.insert(pos, f);
                        }
                        None => continue,
                    }
                    let (Some(m), Some(f)) = (placed, self.thermal.get_mut(&pos)) else {
                        continue;
                    };
                    let p = *self.table.thermal(m);
                    let Some(base) = p.base_temperature else {
                        continue;
                    };
                    set_base_temperature(f, chunk, region, m, p.capacity, base);
                }
            }
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

        let t_start = std::time::Instant::now();
        let schedule = self.begin_tick(world);
        stats.active_chunks = self.active.len();
        let table = Arc::clone(&self.table);
        let ctx = TickCtx {
            table: &table,
            seed: self.seed,
            tick: self.tick,
            ambient: self.ambient,
            hysteresis: self.hysteresis,
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
                    let field = self.thermal.remove(&pos);
                    Some((pos, chunk, state, field))
                })
                .collect();
            let shared: &World = world;
            let outs: Vec<TaskOut> = tasks
                .par_iter_mut()
                .map(|(pos, chunk, state, field)| {
                    let mut view = View {
                        center: chunk,
                        neighbors: neighbors(shared, *pos),
                        origin: pos.origin().0,
                        thermal: field.as_mut(),
                    };
                    automaton::process_chunk(&mut view, &mut state.moved, state.region, &ctx)
                })
                .collect();

            let mut finished = Vec::with_capacity(tasks.len());
            for (pos, chunk, state, field) in tasks {
                stats.cells_scanned += state.region.volume();
                world.attach(pos, chunk);
                self.active.insert(pos, state);
                if let Some(field) = field {
                    self.thermal.insert(pos, field);
                }
                finished.push(pos);
            }
            for (pos, out) in finished.into_iter().zip(outs) {
                stats.moves += out.moves;
                stats.reactions += out.reactions;
                stats.decays += out.decays;
                let origin = pos.origin().0;
                for p in &out.solid_lost {
                    self.integrity.push_natural(origin + p.as_i64vec3());
                }
                self.energy_imbalance += out.imbalance;
                if out.needs_field || !out.heat.is_empty() {
                    self.ensure_field(world, pos);
                    if let Some(f) = self.thermal.get_mut(&pos) {
                        for &(cell, heat, cap) in &out.heat {
                            self.energy_imbalance += f64::from(f.add_heat(cell, heat, cap));
                        }
                    }
                }
                if !out.live.is_empty() {
                    let origin = pos.origin().0;
                    self.mark_dirty(
                        world,
                        origin + out.live.min.as_i64vec3(),
                        origin + out.live.max.as_i64vec3(),
                    );
                }
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
                        if mv.react {
                            stats.reactions += 1;
                        } else {
                            stats.border_moves += 1;
                        }
                    }
                }
            }
        }
        let t_thermal = std::time::Instant::now();
        stats.automaton_ms = (t_thermal - t_start).as_secs_f32() * 1000.0;
        let (transitions, flames) = self.thermal_step(world);
        let t_integrity = std::time::Instant::now();
        stats.thermal_ms = (t_integrity - t_thermal).as_secs_f32() * 1000.0;
        stats.transitions = transitions;
        stats.flames = flames;
        let (visited, detached) = self.integrity_step(world);
        stats.integrity_ms = t_integrity.elapsed().as_secs_f32() * 1000.0;
        stats.integrity_visited = visited;
        stats.detached = detached;
        stats.thermal_chunks = self.thermal.len();
        self.stats = stats;
    }

    /// Structural integrity: cuts floating pieces out of the grid. Returns the voxels
    /// visited and the pieces detached.
    fn integrity_step(&mut self, world: &mut World) -> (u32, u32) {
        let table = Arc::clone(&self.table);
        let pass = self.integrity.run(world, &table);
        let detached = pass.bodies.len() as u32;
        for body in pass.bodies {
            let lo = body.origin.0;
            let hi = lo + body.size.as_i64vec3() - 1;
            // What rested on it falls, and temperature fields lose its voxels.
            self.mark_dirty(world, lo - 1, hi + 1);
            if !self.thermal.is_empty() {
                for (p, m) in body.filled() {
                    self.voxel_removed(WorldPos(lo + p.as_i64vec3()), m);
                }
            }
            tracing::debug!(
                "detached a piece of {} voxels at {:?} ({:?})",
                body.count,
                body.origin,
                body.size
            );
            self.detached.push(body);
        }
        (pass.visited as u32, detached)
    }

    /// Diffusion, growth/sleep of temperature fields, then phase changes and
    /// combustion. Returns the voxels that changed material and the flames emitted.
    fn thermal_step(&mut self, world: &mut World) -> (u32, u32) {
        if self.thermal.is_empty() {
            return (0, 0);
        }
        let _span = tracing::trace_span!("thermal_step", chunks = self.thermal.len()).entered();
        self.thermal.retain(|p, _| world.contains(*p));
        let mut positions: Vec<ChunkPos> = self.thermal.keys().copied().collect();
        positions.sort_by_key(|p| (p.0.z, p.0.y, p.0.x));
        self.diffuse(&positions);
        self.grow_and_sleep(world, &positions);
        self.phase_changes(world, &positions)
    }

    /// Jacobi diffusion: reads the old temperatures everywhere, writes new buffers.
    fn diffuse(&mut self, positions: &[ChunkPos]) {
        let _span = tracing::trace_span!("diffuse").entered();
        while self.thermal_scratch.len() < positions.len() {
            self.thermal_scratch.push(Box::new([0.0; CELLS]));
        }
        let params = DiffusionParams {
            rate: self.diffusion_rate,
            ambient: self.ambient,
        };
        let fields = &self.thermal;
        let lost: f64 = positions
            .par_iter()
            .zip(self.thermal_scratch.par_iter_mut())
            .map(|(pos, out)| {
                let nb = |d: IVec3| fields.get(&ChunkPos(pos.0 + d));
                let neighbors = FACES.map(nb);
                thermal::diffuse(&fields[pos], neighbors, params, out)
            })
            .sum();
        self.energy_imbalance -= lost;
        for (pos, buf) in positions.iter().zip(self.thermal_scratch.iter_mut()) {
            if let Some(f) = self.thermal.get_mut(pos) {
                std::mem::swap(&mut f.temp, buf);
            }
        }
    }

    /// Gives a field to neighbours whose shared face deviates from ambient, and
    /// drops fields that stayed at ambient for `sleep_after` ticks.
    fn grow_and_sleep(&mut self, world: &World, positions: &[ChunkPos]) {
        let mut create = Vec::new();
        let mut idle = Vec::new();
        for pos in positions {
            let f = &self.thermal[pos];
            for d in FACES {
                let n = ChunkPos(pos.0 + d);
                if !self.thermal.contains_key(&n)
                    && world.contains(n)
                    && f.face_deviates(d, self.ambient, self.epsilon)
                {
                    create.push(n);
                }
            }
            if f.max_deviation(self.ambient) < self.epsilon {
                idle.push(*pos);
            }
        }
        for pos in idle {
            let sleep_after = self.sleep_after;
            let Some(f) = self.thermal.get_mut(&pos) else {
                continue;
            };
            f.idle_ticks += 1;
            if f.idle_ticks > sleep_after
                && let Some(f) = self.thermal.remove(&pos)
            {
                let cap: f64 = f.cap.iter().map(|&c| f64::from(c)).sum();
                self.energy_imbalance += f64::from(self.ambient) * cap - f.energy();
            }
        }
        for pos in create {
            if self.thermal.contains_key(&pos) {
                continue;
            }
            if let Some(chunk) = world.chunk(pos) {
                let f = ThermalChunk::new(chunk, &self.table, self.ambient);
                self.thermal.insert(pos, f);
            }
        }
    }

    /// Temperature-driven material changes and flames, chunk by chunk in parallel
    /// (they only touch their own chunk). Returns the voxels changed and the flames.
    fn phase_changes(&mut self, world: &mut World, positions: &[ChunkPos]) -> (u32, u32) {
        let _span = tracing::trace_span!("phase_changes").entered();
        let mut tasks: Vec<_> = positions
            .iter()
            .filter_map(|&pos| {
                let field = self.thermal.remove(&pos)?;
                let Some(chunk) = world.detach(pos) else {
                    self.thermal.insert(pos, field);
                    return None;
                };
                Some((pos, chunk, field))
            })
            .collect();
        let params = TransitionParams {
            hysteresis: self.hysteresis,
            rate: table::chance(self.transition_rate),
            seed: self.seed,
            tick: self.tick,
        };
        let table = Arc::clone(&self.table);
        let outs: Vec<_> = tasks
            .par_iter_mut()
            .map(|(pos, chunk, field)| {
                thermal::transitions(chunk, field, pos.origin().0, &table, params)
            })
            .collect();
        let (mut count, mut flames) = (0, 0);
        for ((pos, chunk, mut field), out) in tasks.into_iter().zip(outs) {
            if out.count + out.flames > 0 {
                field.idle_ticks = 0;
            }
            world.attach(pos, chunk);
            self.thermal.insert(pos, field);
            self.energy_imbalance += out.imbalance;
            count += out.count;
            flames += out.flames;
            let origin = pos.origin().0;
            for p in &out.solid_lost {
                self.integrity.push_natural(origin + p.as_i64vec3());
            }
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
        }
        (count, flames)
    }

    /// Gives `pos` a temperature field (at ambient) if it has none.
    fn ensure_field(&mut self, world: &World, pos: ChunkPos) {
        if !self.thermal.contains_key(&pos)
            && let Some(chunk) = world.chunk(pos)
        {
            let f = ThermalChunk::new(chunk, &self.table, self.ambient);
            self.thermal.insert(pos, f);
        }
    }

    /// Bookkeeping for a voxel at `pos` that changed `old → new` in place (already
    /// written to the world).
    fn voxel_changed(&mut self, world: &World, pos: WorldPos, old: MaterialId, new: MaterialId) {
        if old == new {
            return;
        }
        let (c, l) = pos.to_chunk_local();
        let table = Arc::clone(&self.table);
        let (po, pn) = (table.thermal(old), table.thermal(new));
        match self.thermal.get_mut(&c) {
            Some(f) => {
                self.energy_imbalance +=
                    f.change_voxel(cell_of(l.as_ivec3()), po, pn, f32::INFINITY);
            }
            None => {
                self.energy_imbalance += ambient_change(po, pn, self.ambient);
                if unstable_at(&table, new, self.ambient, self.hysteresis) {
                    // Created from the current composition: nothing to adjust.
                    self.ensure_field(world, c);
                }
            }
        }
    }

    /// Bookkeeping for a voxel of `old` that left the grid (now air) taking its
    /// energy with it: the cell keeps its temperature, no latent heat is released.
    fn voxel_removed(&mut self, pos: WorldPos, old: MaterialId) {
        let (c, l) = pos.to_chunk_local();
        let (po, pa) = (
            *self.table.thermal(old),
            *self.table.thermal(MaterialId::AIR),
        );
        let latent = f64::from(pa.latent - po.latent);
        match self.thermal.get_mut(&c) {
            Some(f) => {
                let cell = cell_of(l.as_ivec3());
                let before = f.energy_of(cell);
                let t = f.temp[cell];
                f.replace_voxel(cell, &po, &pa, t);
                self.energy_imbalance += f.energy_of(cell) - before + latent;
            }
            None => self.energy_imbalance += ambient_change(&po, &pa, self.ambient),
        }
    }

    /// Adds `energy` (capped at `cap_temp`) to the cell containing `pos`, creating
    /// its field if needed.
    fn add_heat(&mut self, world: &World, pos: WorldPos, energy: f32, cap_temp: f32) {
        let (c, l) = pos.to_chunk_local();
        self.ensure_field(world, c);
        if let Some(f) = self.thermal.get_mut(&c) {
            let added = f.add_heat(cell_of(l.as_ivec3()), energy, cap_temp);
            self.energy_imbalance += f64::from(added);
        }
    }

    /// Gives `pos` a temperature field if a voxel of `m` arriving at `t` needs one.
    fn ensure_field_for(&mut self, world: &World, pos: ChunkPos, m: MaterialId, t: f32) {
        if self.thermal.contains_key(&pos) {
            return;
        }
        let needed = (t - self.ambient).abs() > self.epsilon
            || unstable_at(&self.table, m, self.ambient, self.hysteresis);
        if needed && let Some(chunk) = world.chunk(pos) {
            let f = ThermalChunk::new(chunk, &self.table, self.ambient);
            self.thermal.insert(pos, f);
        }
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

    /// Applies a queued border move or reaction if both cells still hold what the
    /// task saw.
    fn apply_move(&mut self, world: &mut World, mv: &Move) -> bool {
        if world.voxel(mv.src) != Some(mv.src_material)
            || world.voxel(mv.dst) != Some(mv.dst_material)
        {
            return false;
        }
        if mv.react {
            world.set_voxel(mv.src, mv.src_into);
            world.set_voxel(mv.dst, mv.dst_into);
            for (pos, old, new) in [
                (mv.src, mv.src_material, mv.src_into),
                (mv.dst, mv.dst_material, mv.dst_into),
            ] {
                if self.table.get(old).kind == Kind::Static
                    && self.table.get(new).kind != Kind::Static
                {
                    self.integrity.push_natural(pos.0);
                }
            }
            self.voxel_changed(world, mv.src, mv.src_material, mv.src_into);
            self.voxel_changed(world, mv.dst, mv.dst_material, mv.dst_into);
            if mv.heat != 0.0 {
                self.add_heat(world, mv.dst, mv.heat, mv.heat_cap);
            }
            let (cpos, local) = mv.dst.to_chunk_local();
            let index = local.index();
            self.activate(cpos).moved[index >> 6] |= 1 << (index & 63);
            for p in [mv.src.0, mv.dst.0] {
                self.mark_dirty(world, p - 1, p + 1);
            }
            return true;
        }
        // Energy travels with the voxels. Fields are created before the swap so their
        // composition does not count the incoming voxel twice.
        let (cs, ls) = mv.src.to_chunk_local();
        let (cd, ld) = mv.dst.to_chunk_local();
        let (ts, td) = (self.temperature_at(mv.src), self.temperature_at(mv.dst));
        self.ensure_field_for(world, cd, mv.src_material, ts);
        self.ensure_field_for(world, cs, mv.dst_material, td);
        world.set_voxel(mv.src, mv.dst_material);
        world.set_voxel(mv.dst, mv.src_material);
        let table = Arc::clone(&self.table);
        let (p_src, p_dst) = (
            table.thermal(mv.src_material),
            table.thermal(mv.dst_material),
        );
        if let Some(f) = self.thermal.get_mut(&cs) {
            f.replace_voxel(cell_of(ls.as_ivec3()), p_src, p_dst, td);
        }
        if let Some(f) = self.thermal.get_mut(&cd) {
            f.replace_voxel(cell_of(ld.as_ivec3()), p_dst, p_src, ts);
        }
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

/// Face directions in the order expected by [`thermal::diffuse`].
const FACES: [IVec3; 6] = [
    IVec3::NEG_X,
    IVec3::X,
    IVec3::NEG_Y,
    IVec3::Y,
    IVec3::NEG_Z,
    IVec3::Z,
];

/// Sets the cells of `region` towards `base` in proportion to how much of their
/// capacity is voxels of `m` (freshly placed material arrives at its base
/// temperature; the rest of the cell keeps its temperature).
fn set_base_temperature(
    f: &mut ThermalChunk,
    chunk: &mv_world::Chunk,
    region: Aabb,
    m: MaterialId,
    capacity: f32,
    base: f32,
) {
    let mut counts = vec![0u32; CELLS];
    for z in region.min.z..=region.max.z {
        for y in region.min.y..=region.max.y {
            for x in region.min.x..=region.max.x {
                let lp = mv_core::LocalPos::new(x as u8, y as u8, z as u8);
                if chunk.get(lp) == m {
                    counts[cell_of(IVec3::new(x, y, z))] += 1;
                }
            }
        }
    }
    for (c, &n) in counts.iter().enumerate() {
        if n > 0 {
            let added = n as f32 * capacity;
            let cap = f.cap[c].max(1e-3);
            let rest = (cap - added).max(0.0);
            f.temp[c] = (f.temp[c] * rest + base * added) / cap;
        }
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
#[cfg(test)]
mod tests_chem;
#[cfg(test)]
mod tests_integrity;
#[cfg(test)]
mod tests_thermal;
